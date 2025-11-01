use clap::Parser;
use log::{error, info};
use rand::Rng;
use serialport::{DataBits, Parity, StopBits};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tokio::time;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{transport::Server, Request, Response, Status};

mod sensor_filter;
use sensor_filter::{FilterType, SensorFilter};

pub mod snowgauge {
    tonic::include_proto!("snowgauge");
}

use snowgauge::{
    snow_gauge_service_server::{SnowGaugeService, SnowGaugeServiceServer},
    Reading, StreamRequest,
};

/// Command line arguments
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Serial port name
    #[arg(long, env = "PORT", default_value = "/dev/ttyS0")]
    port: String,

    /// Turn on debugging output
    #[arg(long, env = "DEBUG")]
    debug: bool,

    /// Address to listen on for gRPC connections
    #[arg(long, env = "LISTEN_ADDR", default_value = "0.0.0.0:7669")]
    listen_addr: String,

    /// Log the distance to stdout
    #[arg(long, env = "LOG_DISTANCE")]
    log: bool,

    /// Enable simulator mode
    #[arg(long, env = "SIMULATOR")]
    simulator: bool,

    /// Base distance for simulator (starting distance in mm)
    #[arg(long, env = "SIMULATOR_BASE_DISTANCE", default_value = "1000.0")]
    simulator_base_distance: f64,

    /// Station name for this snow gauge
    #[arg(long, env = "STATION_NAME", default_value = "snowgauge")]
    station_name: String,

    /// Percentage to trim from each end (0.0-0.5)
    #[arg(long, env = "TRIM_PERCENTAGE", default_value = "0.15")]
    trim_percentage: f64,

    /// Number of readings to collect before averaging
    #[arg(long, env = "BATCH_SIZE", default_value = "30")]
    batch_size: usize,

    /// Filter type: none, exponential, trimmed-mean, or both
    #[arg(long, env = "FILTER_TYPE", default_value = "both", value_parser = clap::value_parser!(FilterType))]
    filter_type: FilterType,

    /// Filter initialization period (number of readings)
    #[arg(long, env = "FILTER_INIT_PERIOD", default_value = "40")]
    filter_init_period: usize,

    /// Filter rate limit (maximum change per reading in mm)
    #[arg(long, env = "FILTER_RATE_LIMIT", default_value = "1.0")]
    filter_rate_limit: f64,

    /// Filter smoothing factor (0.0-1.0, higher = more responsive)
    #[arg(long, env = "FILTER_ALPHA", default_value = "0.2")]
    filter_alpha: f64,
}

/// Client channel structure for streaming
type ClientChannel = mpsc::UnboundedSender<Result<Reading, Status>>;

/// Main service implementation
#[derive(Clone)]
pub struct SnowGaugeServiceImpl {
    client_channels: Arc<RwLock<Vec<ClientChannel>>>,
    station_name: String,
    trim_percentage: f64,
    batch_size: usize,
    filter_type: FilterType,
}

impl SnowGaugeServiceImpl {
    fn new(station_name: String, trim_percentage: f64, batch_size: usize, filter_type: FilterType) -> Self {
        Self {
            client_channels: Arc::new(RwLock::new(Vec::new())),
            station_name,
            trim_percentage,
            batch_size,
            filter_type,
        }
    }

    /// Broadcast reading to all connected clients
    async fn broadcast_reading(&self, reading: Reading) {
        let mut clients = self.client_channels.write().await;

        // Use retain() to atomically filter out disconnected clients
        // This avoids the TOCTOU race condition from collecting indices
        clients.retain(|client| {
            client.send(Ok(reading.clone())).is_ok()
        });
    }

    /// Process readings with trimmed mean
    async fn process_readings(
        &self,
        mut receiver: mpsc::UnboundedReceiver<f64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut batch = Vec::new();

        while let Some(distance) = receiver.recv().await {
            batch.push(distance);

            if batch.len() >= self.batch_size {
                let n = batch.len();
                let average = match self.filter_type {
                    FilterType::TrimmedMean | FilterType::Both => {
                        // Sort with NaN-safe comparison
                        // NaN values are sorted to the end, treating them as larger than any number
                        batch.sort_by(|a, b| {
                            a.partial_cmp(b).unwrap_or_else(|| {
                                match (a.is_nan(), b.is_nan()) {
                                    (false, true) => std::cmp::Ordering::Less,
                                    (true, false) => std::cmp::Ordering::Greater,
                                    _ => std::cmp::Ordering::Equal,
                                }
                            })
                        });

                        // 15% trim on each end removes ~4-5 readings from each tail (8-10 total from batch of 30)
                        // This accounts for sensor noise spikes and environmental interference
                        // while preserving enough data points for statistical validity
                        let trim = (self.trim_percentage * n as f64) as usize;

                        let trimmed: Vec<f64> = if n > 2 * trim {
                            batch[trim..n - trim].to_vec()
                        } else {
                            batch.clone()
                        };

                        let avg = trimmed.iter().sum::<f64>() / trimmed.len() as f64;
                        if self.filter_type == FilterType::Both {
                            info!("Combined filter result: {:.2}mm (from {} pre-filtered readings, trimmed {} from each end)",
                                  avg, n, trim);
                        } else {
                            info!("Trimmed mean: {:.2}mm (from {} readings, trimmed {} from each end)",
                                  avg, n, trim);
                        }
                        avg
                    }
                    FilterType::Exponential | FilterType::None => {
                        // For exponential filter or no filter, just compute simple average
                        // (exponential filtering already happened per-reading)
                        let avg = batch.iter().sum::<f64>() / n as f64;
                        info!("Average distance: {:.2}mm (from {} readings)", avg, n);
                        avg
                    }
                };

                let reading = Reading {
                    station_name: self.station_name.clone(),
                    distance: average as i32,
                    system_uptime: None,
                    application_uptime: None,
                };

                self.broadcast_reading(reading).await;
                batch.clear();
            }
        }

        Ok(())
    }

    /// Read from serial port with exponential backoff on errors
    async fn serial_reader(
        port_name: String,
        sender: mpsc::UnboundedSender<f64>,
        log_distance: bool,
        cancel_token: CancellationToken,
        filter_config: Option<(usize, f64, f64)>, // (init_period, rate_limit, alpha)
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Spawn blocking task for serial I/O and return immediately
        // This task will be cancelled when the cancel_token is triggered
        let cancel_token_clone = cancel_token.clone();
        tokio::task::spawn_blocking(move || {
            let mut backoff = Duration::from_secs(1);
            const MAX_BACKOFF: Duration = Duration::from_secs(60);

            // Initialize filter if configured
            let mut filter = filter_config.map(|(init_period, rate_limit, alpha)| {
                info!("Initializing sensor filter: init_period={}, rate_limit={}mm, alpha={}",
                      init_period, rate_limit, alpha);
                SensorFilter::with_params(init_period, rate_limit, alpha)
            });

            loop {
                if cancel_token_clone.is_cancelled() {
                    info!("Serial reader received shutdown signal");
                    return;
                }

                let settings = serialport::new(&port_name, 9600)
                    .data_bits(DataBits::Eight)
                    .parity(Parity::None)
                    .stop_bits(StopBits::One)
                    .timeout(Duration::from_secs(1)); // Shorter timeout for responsiveness

                match settings.open() {
                    Ok(mut port) => {
                        info!("Serial port opened successfully");
                        backoff = Duration::from_secs(1); // Reset backoff on successful connection

                        let mut buf = [0u8; 6];
                        let mut offset = 0;

                        loop {
                            if cancel_token_clone.is_cancelled() {
                                info!("Serial reader received shutdown signal");
                                return;
                            }

                            match port.read(&mut buf[offset..]) {
                                Ok(n) => {
                                    offset += n;

                                    if offset == 6 {
                                        if buf[0] == b'R' && buf[5] == b'\r' {
                                            let distance_str =
                                                String::from_utf8_lossy(&buf[1..5]);
                                            match distance_str.parse::<f64>() {
                                                Ok(raw_distance) => {
                                                    // Apply filter if enabled
                                                    let distance = if let Some(ref mut f) = filter {
                                                        let filtered = f.update(raw_distance);
                                                        if log_distance {
                                                            info!("Raw: {:.2}mm, Filtered: {:.2}mm (readings: {}/{})",
                                                                  raw_distance, filtered,
                                                                  f.reading_count(), f.reading_count());
                                                        }
                                                        filtered
                                                    } else {
                                                        if log_distance {
                                                            info!("Received measurement: distance={}", raw_distance);
                                                        }
                                                        raw_distance
                                                    };

                                                    if sender.send(distance).is_err() {
                                                        error!("Processing channel closed, stopping serial reader");
                                                        return;
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("Error converting distance to number: {}", e);
                                                }
                                            }
                                        } else {
                                            error!("Invalid data format received: {:?}", buf);
                                            // Try to resynchronize by finding 'R' marker
                                            // Search for 'R' in the buffer to realign
                                            if let Some(pos) = buf.iter().position(|&b| b == b'R') {
                                                // Found 'R' at position pos
                                                // Keep data from 'R' onwards and set offset accordingly
                                                buf.copy_within(pos..6, 0);
                                                offset = 6 - pos;
                                                error!("Resynchronized: found 'R' at position {}, new offset {}", pos, offset);
                                            } else {
                                                // No 'R' found, reset and start fresh
                                                offset = 0;
                                                error!("No sync marker found, resetting buffer");
                                            }
                                            continue;
                                        }
                                        offset = 0;
                                    }
                                }
                                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                                    // Timeout is expected, continue loop to check cancellation
                                    continue;
                                }
                                Err(e) => {
                                    error!("Error reading from serial port: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error opening serial port: {}, retrying in {:?}", e, backoff);
                    }
                }

                // Sleep with cancellation check
                let sleep_until = Instant::now() + backoff;
                while Instant::now() < sleep_until {
                    if cancel_token_clone.is_cancelled() {
                        info!("Serial reader received shutdown signal during backoff");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                backoff = std::cmp::min(backoff * 2, MAX_BACKOFF);
            }
        });

        Ok(())
    }

    /// Simulator generates synthetic snowfall data
    async fn simulator(
        base_distance: f64,
        sender: mpsc::UnboundedSender<f64>,
        log_distance: bool,
        cancel_token: CancellationToken,
        filter_config: Option<(usize, f64, f64)>, // (init_period, rate_limit, alpha)
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting simulator with base_distance={}", base_distance);
        let start_time = Instant::now();

        // Initialize filter if configured
        let mut filter = filter_config.map(|(init_period, rate_limit, alpha)| {
            info!("Initializing sensor filter in simulator: init_period={}, rate_limit={}mm, alpha={}",
                  init_period, rate_limit, alpha);
            SensorFilter::with_params(init_period, rate_limit, alpha)
        });

        let mut interval = time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!("Simulator received shutdown signal");
                    break;
                }
                _ = interval.tick() => {
                    let elapsed = start_time.elapsed();
                    let elapsed_minutes = elapsed.as_secs_f64() / 60.0;

                    // Snowfall rate: 120mm/hour = 2mm/minute
                    let snowfall_mm = elapsed_minutes * 2.0;
                    let base_current_distance = base_distance - snowfall_mm;

                    // Add sinusoidal variations
                    let sine_component = 3.0 * (2.0 * std::f64::consts::PI * elapsed_minutes / 8.0).sin();
                    let fast_sine_component = 1.5 * (2.0 * std::f64::consts::PI * elapsed_minutes / 2.0).sin();

                    // Add random variation (±1mm)
                    let random_component = {
                        let mut rng = rand::thread_rng();
                        (rng.gen::<f64>() - 0.5) * 2.0
                    };

                    let mut current_distance = base_current_distance + sine_component + fast_sine_component + random_component;

                    if current_distance < 0.0 {
                        current_distance = 0.0;
                    }

                    // Apply filter if enabled
                    let distance = if let Some(ref mut f) = filter {
                        let filtered = f.update(current_distance);
                        if log_distance {
                            info!(
                                "Simulated: raw={:.2}mm, filtered={:.2}mm, base={:.2}mm, snowfall={:.2}mm (readings: {})",
                                current_distance, filtered, base_current_distance, snowfall_mm, f.reading_count()
                            );
                        }
                        filtered
                    } else {
                        if log_distance {
                            info!(
                                "Simulated measurement: distance={:.2}, base_distance={:.2}, snowfall_mm={:.2}, variation={:.2}",
                                current_distance,
                                base_current_distance,
                                snowfall_mm,
                                current_distance - base_current_distance
                            );
                        }
                        current_distance
                    };

                    if sender.send(distance).is_err() {
                        error!("Processing channel closed, stopping simulator");
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

#[tonic::async_trait]
impl SnowGaugeService for SnowGaugeServiceImpl {
    type StreamReadingStream = UnboundedReceiverStream<Result<Reading, Status>>;

    async fn stream_reading(
        &self,
        request: Request<StreamRequest>,
    ) -> Result<Response<Self::StreamReadingStream>, Status> {
        let remote_addr = request
            .remote_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        
        info!("Registering new gRPC streaming client [{}]...", remote_addr);

        let (tx, rx) = mpsc::unbounded_channel();
        
        self.client_channels.write().await.push(tx);

        Ok(Response::new(UnboundedReceiverStream::new(rx)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Initialize logger
    if args.debug {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();
    } else {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    }

    // Validate parameters
    if args.trim_percentage < 0.0 || args.trim_percentage > 0.5 {
        error!("trim-percentage must be between 0.0 and 0.5, got {}", args.trim_percentage);
        return Err("Invalid trim-percentage".into());
    }

    if args.batch_size < 10 {
        error!("batch-size must be at least 10, got {}", args.batch_size);
        return Err("Invalid batch-size".into());
    }

    info!("Configuration:");
    info!("  Station name: {}", args.station_name);
    info!("  Filter type: {}", args.filter_type);

    match args.filter_type {
        FilterType::Exponential => {
            info!("  Exponential filter parameters:");
            info!("    - Initialization period: {} readings", args.filter_init_period);
            info!("    - Rate limit: {} mm/reading", args.filter_rate_limit);
            info!("    - Alpha (smoothing): {}", args.filter_alpha);
        }
        FilterType::TrimmedMean => {
            info!("  Trimmed mean parameters:");
            info!("    - Trim percentage: {}% from each end", args.trim_percentage * 100.0);
            info!("    - Batch size: {} readings", args.batch_size);
        }
        FilterType::Both => {
            info!("  Combined filtering (exponential + trimmed mean):");
            info!("    Exponential filter (per-reading):");
            info!("      - Initialization period: {} readings", args.filter_init_period);
            info!("      - Rate limit: {} mm/reading", args.filter_rate_limit);
            info!("      - Alpha (smoothing): {}", args.filter_alpha);
            info!("    Trimmed mean (batch):");
            info!("      - Trim percentage: {}% from each end", args.trim_percentage * 100.0);
            info!("      - Batch size: {} readings", args.batch_size);
        }
        FilterType::None => {
            info!("  No filtering applied - using raw readings");
        }
    }

    // Build filter configuration for exponential filter (used in Both and Exponential modes)
    let filter_config = if args.filter_type == FilterType::Exponential || args.filter_type == FilterType::Both {
        Some((args.filter_init_period, args.filter_rate_limit, args.filter_alpha))
    } else {
        None
    };

    let (tx, rx) = mpsc::unbounded_channel();

    let service = Arc::new(SnowGaugeServiceImpl::new(
        args.station_name.clone(),
        args.trim_percentage,
        args.batch_size,
        args.filter_type,
    ));

    // Create cancellation token for coordinated shutdown
    let cancel_token = CancellationToken::new();

    // Start the processing task
    let service_clone = Arc::clone(&service);
    let processing_task = tokio::spawn(async move {
        if let Err(e) = service_clone.process_readings(rx).await {
            error!("Error processing readings: {}", e);
        }
    });

    // Start serial reader or simulator
    let data_source_task = if args.simulator {
        let cancel_token_clone = cancel_token.clone();
        tokio::spawn(async move {
            if let Err(e) = SnowGaugeServiceImpl::simulator(
                args.simulator_base_distance,
                tx,
                args.log,
                cancel_token_clone,
                filter_config,
            ).await {
                error!("Simulator error: {}", e);
            }
        })
    } else {
        let port_name = args.port.clone();
        let log_distance = args.log;
        let cancel_token_clone = cancel_token.clone();
        tokio::spawn(async move {
            if let Err(e) = SnowGaugeServiceImpl::serial_reader(
                port_name.clone(),
                tx,
                log_distance,
                cancel_token_clone,
                filter_config,
            ).await {
                error!("Serial reader error: {}", e);
            }
        })
    };

    if args.simulator {
        info!("Started simulator with base_distance={}", args.simulator_base_distance);
    } else {
        info!("Started serial reader on port {}", args.port);
    }

    // Start gRPC server with graceful shutdown
    let addr = args.listen_addr.parse()?;
    info!("gRPC server listening on {}", addr);

    // Enable gRPC reflection for easier debugging with grpcurl
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(include_bytes!("../target/snowgauge_descriptor.bin"))
        .build_v1()?;

    Server::builder()
        .add_service(SnowGaugeServiceServer::new((*service).clone()))
        .add_service(reflection_service)
        .serve_with_shutdown(addr, async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for shutdown signal");
            info!("Shutdown signal received, gracefully stopping...");
            cancel_token.cancel();
        })
        .await?;

    info!("Server stopped, waiting for background tasks to complete...");

    // Wait for the data source task (serial reader or simulator) to finish
    // When it completes, tx is dropped, which closes the channel
    if let Err(e) = data_source_task.await {
        error!("Data source task panicked: {}", e);
    }

    // Wait for the processing task to finish
    // It will complete once the channel is closed
    if let Err(e) = processing_task.await {
        error!("Processing task panicked: {}", e);
    }

    info!("All tasks completed, exiting");
    Ok(())
}