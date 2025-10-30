# SnowGauge Rust Port

Rust implementation of the SnowGauge gRPC service for reading snow depth measurements from ultrasonic sensors.

## Build

```bash
cargo build --release
```

## Run

### With Serial Port
```bash
cargo run -- --port /dev/ttyS0 --listen-addr 0.0.0.0:7669
```

### With Simulator
```bash
cargo run -- --simulator --simulator-base-distance 1000.0 --listen-addr 0.0.0.0:7669 --log
```

## Command Line Options

### Basic Options
- `--port`: Serial port name (default: /dev/ttyS0)
- `--debug`: Enable debug logging
- `--listen-addr`: gRPC server address (default: 0.0.0.0:7669)
- `--log`: Log distance measurements to stdout

### Simulator Options
- `--simulator`: Enable simulator mode
- `--simulator-base-distance`: Starting distance in mm for simulator (default: 1000.0)

### Station Configuration
- `--station-name`: Station name for this snow gauge (default: snowgauge)

### Filter Configuration
- `--filter-type`: Filter type: none, exponential, trimmed-mean, or both (default: both)
- `--batch-size`: Number of readings to collect before averaging (default: 30, minimum: 10)
- `--trim-percentage`: Percentage to trim from each end for trimmed-mean filter (default: 0.15, range: 0.0-0.5)

### Exponential Filter Options
- `--filter-init-period`: Filter initialization period in number of readings (default: 40)
- `--filter-rate-limit`: Maximum change per reading in mm (default: 1.0)
- `--filter-alpha`: Filter smoothing factor, higher = more responsive (default: 0.2, range: 0.0-1.0)

All options can also be set via environment variables:
- `PORT`
- `DEBUG`
- `LISTEN_ADDR`
- `LOG_DISTANCE`
- `SIMULATOR`
- `SIMULATOR_BASE_DISTANCE`
- `STATION_NAME`
- `FILTER_TYPE`
- `BATCH_SIZE`
- `TRIM_PERCENTAGE`
- `FILTER_INIT_PERIOD`
- `FILTER_RATE_LIMIT`
- `FILTER_ALPHA`