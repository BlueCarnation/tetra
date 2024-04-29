use hackrfone::{HackRfOne, UnknownMode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
struct SignalData {
    is_signal: String,
    signal_strength: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    instant_scan: bool,
    start_after_duration: u64,
    scan_duration: u64,
}

fn load_config(config_path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let file = File::open(config_path)?;
    let reader = std::io::BufReader::new(file);
    let config = serde_json::from_reader(reader)?;
    Ok(config)
}

fn scan_freq(
    mut radio: HackRfOne<UnknownMode>,
    frequency: u64,
    sample_rate: u32,
    duration: Duration,
) -> Vec<u8> {
    radio.set_freq(frequency).expect("Failed to set frequency");
    radio
        .set_sample_rate(sample_rate, 1)
        .expect("Failed to set sample rate");
    radio
        .set_amp_enable(true)
        .expect("Failed to enable amplifier");
    radio.set_lna_gain(24).expect("Failed to set LNA gain");
    radio.set_vga_gain(28).expect("Failed to set VGA gain");

    // Enter RX mode and receive samples
    let mut radio_rx = radio.into_rx_mode().expect("Failed to enter RX mode");

    let start_time = Instant::now();
    let mut raw_samples = Vec::new();

    loop {
        let samples = radio_rx.rx().expect("Failed to receive samples");
        raw_samples.extend(samples);

        if start_time.elapsed() >= duration {
            break;
        }
    }

    raw_samples
}

fn analyze_samples(samples: Vec<u8>) -> Vec<f64> {
    samples
        .iter()
        .map(|&sample| {
            let sample_f64 = sample as f64;
            if sample_f64 > 0.0 {
                20.0 * sample_f64.log10()
            } else {
                0.0
            }
        })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config("config.json")?;

    if config.instant_scan {
        run_instant_scan().await?;
    } else {
        run_scan_over_duration(config.start_after_duration, config.scan_duration).await?;
    }

    Ok(())
}

async fn run_instant_scan() -> Result<(), Box<dyn std::error::Error>> {
    println!("Running instant scan...");

    let mut results: HashMap<String, Value> = HashMap::new();
    let mut count = 1;

    let start_freq = 380_000_000u64;
    let end_freq = 420_000_000u64;
    let step = 1_000_000u64;
    let sample_rate = 1_000_000u32;
    let duration = Duration::from_secs(1);

    for freq in (start_freq..=end_freq).step_by(step as usize) {
        let radio = HackRfOne::new().expect("Failed to open HackRF One");
        let raw_samples = scan_freq(radio, freq, sample_rate, duration);

        println!("Scanning frequency: {} MHz", freq as f64 / 1_000_000.0);
        println!("Received {} samples", raw_samples.len());

        let signal_strengths_db = analyze_samples((raw_samples).clone());
        let max_strength = signal_strengths_db
            .iter()
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .copied();

        if let Some(max) = max_strength {
            if max > 49.0 {
                println!("Signal detected: true");
                results.insert(
                    count.to_string(),
                    json!({
                        "freq": freq as f64 / 1_000_000.0,
                        "strength": max,
                        "sample_count": raw_samples.len()
                    }),
                );
                count += 1;
            } else {
                println!(
                    "Signal below threshold detected at {} MHz with strength {:.2} dB",
                    freq as f64 / 1_000_000.0,
                    max
                );
            }
        } else {
            println!("No signal detected.");
        }
    }

    if results.is_empty() {
        results.insert(
            "1".to_string(),
            json!({"freq": 0, "max_strength": 0, "sample_count": 0}),
        );
    }

    let json = serde_json::to_string_pretty(&results)?;
    println!("{}", json);

    let mut file = File::create("tetra_instantdata.json")?;
    file.write_all(json.as_bytes())?;

    Ok(())
}

async fn run_scan_over_duration(start_after_duration: u64, scan_duration: u64) -> Result<(), Box<dyn std::error::Error>> {
    for i in (1..=start_after_duration).rev() {
        println!("Scan starts in {} seconds", i);
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }

    println!("Starting scan for {} seconds...", scan_duration);
    let scan_start_time = Instant::now();

    let start_freq = 380_000_000u64;
    let end_freq = 420_000_000u64;
    let step = 1_000_000u64;
    let sample_rate = 1_000_000u32;
    let duration_per_freq = Duration::from_secs(1);

    // Initialize an empty vector to store frequency data
    let mut freq_data_vec: Vec<Value> = vec![];
    let mut freq_id_map: HashMap<u64, usize> = HashMap::new(); // Maps frequency to ID

    while Instant::now().duration_since(scan_start_time) < Duration::from_secs(scan_duration) {
        for freq in (start_freq..=end_freq).step_by(step as usize) {
            if Instant::now().duration_since(scan_start_time) >= Duration::from_secs(scan_duration) {
                break; // End of the duration scan
            }

            let radio = HackRfOne::new().expect("Failed to open HackRF One");
            let raw_samples = scan_freq(radio, freq, sample_rate, duration_per_freq);
            let signal_strengths_db = analyze_samples(raw_samples.clone());
            let max_strength = signal_strengths_db.iter().max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).copied();

            if let Some(max) = max_strength {
                if max > 49.0 {
                    let current_time = Instant::now().duration_since(scan_start_time).as_secs();
                    let freq_index = *freq_id_map.entry(freq).or_insert_with(|| {
                        let new_index = freq_data_vec.len();
                        freq_data_vec.push(json!({
                            "freq": freq as f64 / 1_000_000.0,
                            "strength": max,
                            "sample_count": raw_samples.len(),
                            "tetra_durations": format!("{}-{}", current_time, current_time + 1)

                        }));
                        new_index
                    });

                    let detection = &mut freq_data_vec[freq_index];
                    let durations_str = detection["tetra_durations"].as_str().unwrap_or("");
                    let new_duration = if durations_str.is_empty() {
                        format!("{}-{}", current_time, current_time + 1)
                    } else {
                        format!("{},{}-{}", durations_str, current_time, current_time + 1)
                    };

                    detection["tetra_durations"] = json!(new_duration);
                }
            }
        }
    }

    // Organize results by frequency order
    let results: Value = freq_data_vec.into_iter().enumerate().map(|(id, data)| (id.to_string(), data)).collect();

    let json = serde_json::to_string_pretty(&results)?;
    println!("{}", json);
    let mut file = File::create("tetra_scheduledata.json")?;
    file.write_all(json.as_bytes())?;

    Ok(())
}
