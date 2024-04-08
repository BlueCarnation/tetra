use hackrfone::{HackRfOne, UnknownMode};
use serde::{Serialize, Deserialize};
use std::time::{Instant, Duration};
use std::fs::File;
use std::io::Write;
use std::collections::HashMap;
use tokio::time::sleep;

#[derive(Serialize, Deserialize)]
struct SignalData {
    frequency: f64,
    is_signal_detected: bool,
    max_signal_strength: f64,
    zwave_durations: String,
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

fn scan_freq(mut radio: HackRfOne<UnknownMode>, frequency: u64, sample_rate: u32, duration: Duration) -> Vec<u8> {
    radio.set_freq(frequency).expect("Failed to set frequency");
    radio.set_sample_rate(sample_rate, 1).expect("Failed to set sample rate");
    radio.set_amp_enable(true).expect("Failed to enable amplifier");
    radio.set_lna_gain(16).expect("Failed to set LNA gain");
    radio.set_vga_gain(20).expect("Failed to set VGA gain");

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
    samples.iter().map(|&sample| {
        let sample_f64 = sample as f64;
        if sample_f64 > 0.0 {
            20.0 * sample_f64.log10()
        } else {
            0.0
        }
    }).collect()
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

pub async fn run_instant_scan() -> Result<bool, Box<dyn std::error::Error>>  {
    println!("Running instant scan...");

    // define the 2 frequancy for EU Z-Wave
    let frequency = 868_400_000u64; // 868.4 MHz

    // define the bandwidth and sample rate for each scan
    let sample_rate = 10_000_000u32; // 10 MS/s

    // define the duration for each scan
    let duration = Duration::from_secs(5); // total of 20 seconds for each scan

    let radio: HackRfOne<UnknownMode> = HackRfOne::new().expect("Failed to open HackRF One");
    let raw_samples: Vec<u8> = scan_freq(radio, frequency, sample_rate, duration);

    
    // Print the number of samples received
    println!("Received {} samples", raw_samples.len());

    let signal_strengths_db = analyze_samples(raw_samples);

    let max_strength = signal_strengths_db.iter().max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    match max_strength {
        Some(max) => println!("The highest strength found is: {}", max),
        None => println!("The vector is empty"),
    }
    
    if max_strength > Some(&50.0) {
        println!("Z-Wave signal detected");
    } else {
        println!("No Z-Wave signal detected");
    }

    let data = SignalData {
        frequency: frequency as f64,
        is_signal_detected: max_strength.map_or(false, |&strength| strength > 50.0),
        max_signal_strength: *max_strength.unwrap_or(&0.0),
        zwave_durations: String::from("5"),
    };


    let mut map = HashMap::new();
    map.insert("1", data);
    
    let json = serde_json::to_string(&map).expect("Failed to serialize data");
    println!("{}", json);
    
    let mut file = File::create("zwave_instantdata.json").expect("Failed to create file");
    file.write_all(json.as_bytes()).expect("Failed to write data");


    if json == "{}" {
        Ok(false)
    } else {
        Ok(true)
    }
}

async fn run_scan_over_duration(start_after_duration: u64, scan_duration: u64) -> Result<(), Box<dyn std::error::Error>> {
    for i in (1..=start_after_duration).rev() {
        println!("Scan starts in {} seconds", i);
        sleep(Duration::from_secs(1)).await;
    }

    println!("Starting scan for {} seconds...", scan_duration);

    let frequency = 868_400_000u64;
    let sample_rate = 10_000_000u32;
    let scan_start_time = Instant::now();
    let mut intervals = Vec::new();
    let mut max_strength = 0.0_f64;
    let mut signal_detected = false;

    while Instant::now().duration_since(scan_start_time) < Duration::from_secs(scan_duration) {
        if Instant::now().duration_since(scan_start_time) + Duration::from_secs(1) > Duration::from_secs(scan_duration) {
            // If the remaining time is less than 1 second, break the loop
            break;
        }

        let radio = HackRfOne::new().expect("Failed to open HackRF One");
        let raw_samples = scan_freq(radio, frequency, sample_rate, Duration::from_secs(1));
        let signal_strengths = analyze_samples(raw_samples);

        if let Some(&strength) = signal_strengths.iter().max_by(|a, b| a.partial_cmp(b).unwrap()) {
            if strength > 50.0 { // Threshold for signal detection
                signal_detected = true;
                max_strength = max_strength.max(strength);
                let elapsed = Instant::now().duration_since(scan_start_time).as_secs();
                intervals.push((elapsed, elapsed + 1));
            }
        }
    }

    let merged_intervals = merge_intervals(intervals);
    let durations_str = merged_intervals.iter()
        .map(|&(start, end)| format!("{}-{}", start, end))
        .collect::<Vec<_>>()
        .join(",");

    let result = SignalData {
        frequency: frequency as f64 / 1_000_000.0,
        is_signal_detected: signal_detected,
        max_signal_strength: max_strength,
        zwave_durations: durations_str,
    };

    let json = serde_json::to_string_pretty(&result)?;
    println!("{}", json);

    let mut file = File::create("zwave_scheduledata.json")?;
    file.write_all(json.as_bytes())?;

    Ok(())
}

fn merge_intervals(mut intervals: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    if intervals.is_empty() {
        return Vec::new();
    }

    intervals.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut merged = vec![intervals[0]];

    for &(start, end) in &intervals[1..] {
        let last = merged.last_mut().unwrap();

        // Fusionnez si l'intervalle de départ est dans les 5 secondes suivant la fin du dernier intervalle fusionné
        if start <= last.1 + 5 {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }

    merged
}