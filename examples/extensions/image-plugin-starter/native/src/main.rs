//! A normal independent executable implementing PhotoCraft's image-worker contract.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

const MAX_JOB_BYTES: u64 = 256 * 1024;
const MAX_SIDE: u32 = 32_768;
const MAX_PIXELS: u64 = 100_000_000;
const MAX_WORKING_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug)]
struct Job {
    input: PathBuf,
    output: PathBuf,
    width: u32,
    height: u32,
    gain: f64,
}

fn geometry(width: u32, height: u32) -> Result<usize, String> {
    let pixels = u64::from(width).checked_mul(u64::from(height)).ok_or("Image dimensions overflow.")?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || pixels > MAX_PIXELS {
        return Err("Image exceeds the side or pixel limit.".to_owned());
    }
    usize::try_from(pixels.checked_mul(4).ok_or("Image byte count overflows.")?).map_err(|_| "Image does not fit this platform.".to_owned())
}

fn parse_job(value: &Value) -> Result<Job, String> {
    let object = value.as_object().ok_or("Job must be an object.")?;
    if object.keys().any(|key| !matches!(key.as_str(), "apiVersion" | "input" | "output" | "options" | "outputSize" | "colorSpace")) {
        return Err("Unknown job field.".to_owned());
    }
    if value.get("apiVersion").and_then(Value::as_u64) != Some(1) || value.get("colorSpace").and_then(Value::as_str) != Some("srgb") {
        return Err("Expected image job API 1 with sRGB pixels.".to_owned());
    }
    let path = |key| -> Result<PathBuf, String> {
        let text = value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).ok_or("Missing job path.")?;
        let path = PathBuf::from(text);
        if !path.is_absolute() {
            return Err("Job paths must be absolute.".to_owned());
        }
        Ok(path)
    };
    let input = path("input")?;
    let output = path("output")?;
    if input == output {
        return Err("Input and output paths must differ.".to_owned());
    }
    let size = value.get("outputSize").and_then(Value::as_object).ok_or("Missing outputSize object.")?;
    if size.len() != 2 {
        return Err("outputSize must contain only width and height.".to_owned());
    }
    let dimension =
        |key| size.get(key).and_then(Value::as_u64).and_then(|number| u32::try_from(number).ok()).ok_or_else(|| "Invalid output dimension.".to_owned());
    let width = dimension("width")?;
    let height = dimension("height")?;
    geometry(width, height)?;
    let options = value.get("options").and_then(Value::as_object).ok_or("Options must be an object.")?;
    if options.keys().any(|key| key != "gain") {
        return Err("Unknown gain option.".to_owned());
    }
    let gain = match options.get("gain") {
        None => 1.25,
        Some(value) => value.as_f64().filter(|gain| gain.is_finite() && (0.0..=2.0).contains(gain)).ok_or("Gain must be from 0 to 2.")?,
    };
    Ok(Job { input, output, width, height, gain })
}

fn report(progress: u8, message: &str) {
    // A broken diagnostic pipe must not panic or contaminate the image output.
    let _ = writeln!(std::io::stderr().lock(), "{}", json!({"progress": progress, "message": message}));
}

fn cancelled(flag: &AtomicBool) -> Result<(), String> {
    if flag.load(Ordering::Relaxed) { Err("Processing cancelled.".to_owned()) } else { Ok(()) }
}

fn gain_pixels(source: &[u8], source_width: u32, source_height: u32, job: &Job, flag: &AtomicBool) -> Result<Vec<u8>, String> {
    let source_bytes = geometry(source_width, source_height)?;
    let output_bytes = geometry(job.width, job.height)?;
    if source.len() != source_bytes {
        return Err("Source buffer does not match its dimensions.".to_owned());
    }
    let estimated = (source_bytes as u64).checked_add(output_bytes as u64).and_then(|bytes| bytes.checked_mul(2)).ok_or("Working memory overflows.")?;
    if estimated > MAX_WORKING_BYTES {
        return Err("Estimated working memory exceeds 2 GiB.".to_owned());
    }
    let mut output = Vec::new();
    output.try_reserve_exact(output_bytes).map_err(|_| "Not enough memory for the output.")?;
    output.resize(output_bytes, 0);
    for y in 0..job.height {
        cancelled(flag)?;
        let source_y = u64::from(y) * u64::from(source_height) / u64::from(job.height);
        for x in 0..job.width {
            // Nearest-neighbor scaling keeps this example independent and easy to inspect.
            let source_x = u64::from(x) * u64::from(source_width) / u64::from(job.width);
            let index = ((source_y * u64::from(source_width) + source_x) * 4) as usize;
            let destination = ((u64::from(y) * u64::from(job.width) + u64::from(x)) * 4) as usize;
            let source_pixel = source.get(index..index.saturating_add(4)).ok_or("Source pixel is out of bounds.")?;
            let pixel = output.get_mut(destination..destination.saturating_add(4)).ok_or("Output pixel is out of bounds.")?;
            for (index, (channel, sample)) in pixel.iter_mut().zip(source_pixel).enumerate() {
                *channel = if index == 3 { *sample } else { (f64::from(*sample) * job.gain).round().clamp(0.0, 255.0) as u8 };
            }
        }
        if y % 128 == 0 {
            report((10 + u64::from(y) * 75 / u64::from(job.height)) as u8, "Adjusting pixels");
        }
    }
    Ok(output)
}

fn process(job: &Job, flag: &AtomicBool) -> Result<(), String> {
    cancelled(flag)?;
    report(2, "Reading source image");
    let file = File::open(&job.input).map_err(|error| format!("Could not open source PNG: {error}"))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_limits(png::Limits { bytes: MAX_WORKING_BYTES as usize });
    let mut reader = decoder.read_info().map_err(|error| format!("Invalid source PNG: {error}"))?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    let bytes = geometry(width, height)?;
    if info.bit_depth != png::BitDepth::Eight || info.color_type != png::ColorType::Rgba || info.animation_control.is_some() {
        return Err("Expected one 8-bit RGBA sRGB PNG from the host.".to_owned());
    }
    if reader.output_buffer_size() != Some(bytes) {
        return Err("Unexpected decoded image size.".to_owned());
    }
    let mut input = Vec::new();
    input.try_reserve_exact(bytes).map_err(|_| "Not enough memory for the source.")?;
    input.resize(bytes, 0);
    reader.next_frame(&mut input).map_err(|error| format!("Could not decode source PNG: {error}"))?;
    let output = gain_pixels(&input, width, height, job, flag)?;
    cancelled(flag)?;
    report(90, "Writing result");
    let file = File::create(&job.output).map_err(|error| format!("Could not create result PNG: {error}"))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), job.width, job.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header().map_err(|error| format!("Could not write PNG header: {error}"))?;
    writer.write_image_data(&output).map_err(|error| format!("Could not write result pixels: {error}"))?;
    writer.finish().map_err(|error| format!("Could not finish result PNG: {error}"))?;
    cancelled(flag)?;
    report(100, "Result ready");
    Ok(())
}

fn run(path: &Path, flag: &AtomicBool) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| format!("Could not read job: {error}"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_JOB_BYTES {
        return Err("Job file exceeds 256 KiB or is not a file.".to_owned());
    }
    let mut bytes = Vec::new();
    File::open(path).map_err(|error| error.to_string())?.take(MAX_JOB_BYTES + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_JOB_BYTES {
        return Err("Job file changed or exceeds 256 KiB.".to_owned());
    }
    let value = serde_json::from_slice(&bytes).map_err(|error| format!("Invalid job JSON: {error}"))?;
    process(&parse_job(&value)?, flag)
}

fn main() -> std::process::ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 || arguments.first().is_none_or(|value| value != "--job") {
        report(0, "error: expected --job followed by a job.json path");
        return std::process::ExitCode::FAILURE;
    }
    let Some(path) = arguments.get(1) else { return std::process::ExitCode::FAILURE };
    let cancel = Arc::new(AtomicBool::new(false));
    let listener_flag = Arc::clone(&cancel);
    let listener = std::thread::Builder::new().name("host-cancellation".to_owned()).spawn(move || {
        let mut input = std::io::stdin().lock();
        let mut line = String::new();
        // Limit stdin memory even when invoked outside PhotoCraft.
        let _ = input.by_ref().take(128).read_line(&mut line);
        listener_flag.store(true, Ordering::Relaxed);
    });
    if listener.is_err() {
        report(0, "error: could not start cancellation listener");
        return std::process::ExitCode::FAILURE;
    }
    match run(Path::new(path), &cancel) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            report(0, &format!("error: {error}"));
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(gain: f64, width: u32, height: u32) -> Job {
        Job { input: PathBuf::new(), output: PathBuf::new(), gain, width, height }
    }

    #[test]
    fn gain_changes_rgb_and_preserves_alpha() {
        let pixels = gain_pixels(&[10, 100, 200, 37], 1, 1, &job(1.5, 1, 1), &AtomicBool::new(false)).unwrap();
        assert_eq!(pixels, [15, 150, 255, 37]);
    }

    #[test]
    fn requested_output_size_is_honored() {
        let pixels = gain_pixels(&[10, 20, 30, 40, 50, 60, 70, 80], 2, 1, &job(1.0, 4, 2), &AtomicBool::new(false)).unwrap();
        assert_eq!(pixels.len(), 32);
        assert_eq!(&pixels[..16], &[10, 20, 30, 40, 10, 20, 30, 40, 50, 60, 70, 80, 50, 60, 70, 80]);
        assert_eq!(&pixels[..16], &pixels[16..]);
    }

    #[test]
    fn cancellation_and_invalid_geometry_fail_before_a_result() {
        assert!(gain_pixels(&[0; 4], 1, 1, &job(1.0, 1, 1), &AtomicBool::new(true)).is_err());
        for (width, height) in [(0, 1), (1, 0), (32_769, 1), (20_000, 20_000), (u32::MAX, u32::MAX)] {
            assert!(geometry(width, height).is_err());
        }
        assert!(gain_pixels(&[], 1, 1, &job(1.0, 1, 1), &AtomicBool::new(false)).is_err());
    }

    fn valid_json() -> Value {
        let root = std::env::temp_dir();
        json!({"apiVersion":1,"input":root.join("source.png"),"output":root.join("result.png"),
            "colorSpace":"srgb","outputSize":{"width":2,"height":1},"options":{"gain":1.5}})
    }

    #[test]
    fn job_parses_the_versioned_contract() {
        let parsed = parse_job(&valid_json()).unwrap();
        assert_eq!((parsed.width, parsed.height, parsed.gain), (2, 1, 1.5));
    }

    #[test]
    fn malformed_options_paths_and_versions_are_rejected() {
        for (key, value) in [
            ("apiVersion", json!(2)),
            ("input", json!("relative.png")),
            ("colorSpace", json!("display-p3")),
            ("options", json!({"gain": 4})),
            ("options", json!({"gain": "1"})),
            ("options", json!({"unknown": 1})),
            ("outputSize", json!({"width": 0, "height": 1})),
            ("outputSize", json!({"width": 1, "height": 1, "extra": 1})),
        ] {
            let mut value_object = valid_json();
            value_object[key] = value;
            assert!(parse_job(&value_object).is_err());
        }
    }

    #[test]
    fn actual_png_processing_writes_srgb_rgba_and_preserves_alpha() {
        let directory = std::env::temp_dir().join(format!(
            "photocraft-image-gain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let input_path = directory.join("source.png");
        let output_path = directory.join("result.png");
        {
            let mut encoder = png::Encoder::new(BufWriter::new(File::create(&input_path).unwrap()), 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[20, 100, 180, 45]).unwrap();
            writer.finish().unwrap();
        }
        let actual_job = Job { input: input_path.clone(), output: output_path.clone(), width: 2, height: 1, gain: 1.5 };
        process(&actual_job, &AtomicBool::new(false)).unwrap();
        {
            let mut result = png::Decoder::new(BufReader::new(File::open(&output_path).unwrap())).read_info().unwrap();
            assert_eq!((result.info().width, result.info().height), (2, 1));
            assert_eq!(result.info().bit_depth, png::BitDepth::Eight);
            assert_eq!(result.info().color_type, png::ColorType::Rgba);
            assert!(result.info().srgb.is_some());
            let mut pixels = vec![0; result.output_buffer_size().unwrap()];
            result.next_frame(&mut pixels).unwrap();
            assert_eq!(pixels, [30, 150, 255, 45, 30, 150, 255, 45]);
        }
        fs::remove_file(input_path).unwrap();
        fs::remove_file(output_path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
