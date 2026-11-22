use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::Resampler;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

pub const RATE: usize = 16_000;
pub const MAX_SECONDS: usize = 120;
const MAX_SAMPLES: usize = RATE * MAX_SECONDS;

pub struct Capture {
    stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
    rate: usize,
}

impl Capture {
    pub fn start(level: Arc<AtomicU32>) -> Result<Self, String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or("Мікрофон не знайдено")?;
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let config: cpal::StreamConfig = supported.clone().into();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let error = Arc::new(Mutex::new(None));
        let channels = config.channels as usize;
        let rate = config.sample_rate.0 as usize;
        macro_rules! stream {
            ($ty:ty, $convert:expr) => {{
                let samples = samples.clone();
                let error = error.clone();
                let level = level.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[$ty], _| {
                        let mut buffer = samples.lock().unwrap();
                        let mut power = 0.0;
                        let mut count = 0;
                        for frame in data.chunks_exact(channels) {
                            let mono = frame.iter().map(|&sample| ($convert)(sample)).sum::<f32>()
                                / channels as f32;
                            power += mono * mono;
                            count += 1;
                            if buffer.len() < rate * MAX_SECONDS {
                                buffer.push(mono);
                            }
                        }
                        let rms = (power / count.max(1) as f32).sqrt();
                        level.store((rms * 12.0).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                    },
                    move |e| {
                        *error.lock().unwrap() = Some(e.to_string());
                    },
                    None,
                )
            }};
        }
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => stream!(f32, |v: f32| v),
            cpal::SampleFormat::I16 => stream!(i16, |v: i16| v as f32 / 32768.0),
            cpal::SampleFormat::U16 => stream!(u16, |v: u16| (v as f32 - 32768.0) / 32768.0),
            format => return Err(format!("Непідтримуваний формат мікрофона: {format}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            stream,
            samples,
            error,
            rate,
        })
    }

    pub fn finish(self) -> Result<Vec<f32>, String> {
        let _ = self.stream.pause();
        drop(self.stream);
        if let Some(error) = self.error.lock().unwrap().take() {
            return Err(error);
        }
        let input = std::mem::take(&mut *self.samples.lock().unwrap());
        if input.len() >= self.rate * MAX_SECONDS {
            return Err("Запис задовгий: максимум 2 хвилини".into());
        }
        resample(&input, self.rate)
    }

    pub fn snapshot(&self) -> Result<Vec<f32>, String> {
        snapshot_samples(&self.samples, &self.error, self.rate)
    }
}

fn snapshot_samples(
    samples: &Mutex<Vec<f32>>,
    error: &Mutex<Option<String>>,
    rate: usize,
) -> Result<Vec<f32>, String> {
    let capture_error = error.lock().unwrap().clone();
    if let Some(error) = capture_error {
        return Err(error);
    }

    let input = {
        let samples = samples.lock().unwrap();
        if samples.len() >= rate * MAX_SECONDS {
            return Err("Запис задовгий: максимум 2 хвилини".into());
        }
        samples.clone()
    };

    resample(&input, rate)
}

pub fn resample(input: &[f32], rate: usize) -> Result<Vec<f32>, String> {
    if rate == 0 {
        return Err("Некоректна частота звуку".into());
    }
    if rate == RATE {
        return Ok(input.to_vec());
    }
    let mut resampler =
        rubato::FftFixedInOut::<f32>::new(rate, RATE, 1024, 1).map_err(|e| e.to_string())?;
    let delay = resampler.output_delay();
    let wanted = input.len() * RATE / rate;
    let block = resampler.input_frames_next();
    let mut output = Vec::new();
    let mut cursor = 0;
    while output.len() < wanted + delay {
        let mut chunk = vec![0.0; block];
        let available = input.len().saturating_sub(cursor).min(block);
        chunk[..available]
            .copy_from_slice(&input[cursor.min(input.len())..cursor.min(input.len()) + available]);
        output.extend(
            resampler
                .process(&[chunk], None)
                .map_err(|e| e.to_string())?
                .remove(0),
        );
        cursor += available;
    }
    Ok(output[delay..delay + wanted].to_vec())
}

pub fn wav(samples: &[f32]) -> Vec<u8> {
    let length = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + length as usize);
    bytes.extend(b"RIFF");
    bytes.extend((36 + length).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend((RATE as u32).to_le_bytes());
    bytes.extend((RATE as u32 * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(length.to_le_bytes());
    for &sample in samples {
        bytes.extend(((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    bytes
}

pub fn read_wav(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Потрібен WAV-файл".into());
    }
    let mut offset = 12;
    let mut format = None;
    let mut data = None;
    while offset + 8 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let end = start
            .checked_add(length)
            .filter(|&end| end <= bytes.len())
            .ok_or("Пошкоджений WAV")?;
        match &bytes[offset..offset + 4] {
            b"fmt " if length >= 16 => {
                format = Some((
                    u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap()),
                    u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap()),
                    u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()),
                    u16::from_le_bytes(bytes[start + 14..start + 16].try_into().unwrap()),
                ));
            }
            b"data" => data = Some(&bytes[start..end]),
            _ => {}
        }
        offset = end + length % 2;
    }
    if format != Some((1, 1, RATE as u32, 16)) {
        return Err("Потрібен PCM16 WAV, 16 кГц, моно".into());
    }
    let data = data.ok_or("WAV не містить звуку")?;
    if data.len() % 2 != 0 || data.len() / 2 > MAX_SAMPLES {
        return Err("Некоректна або завелика довжина WAV".into());
    }
    let samples: Vec<_> = data
        .chunks_exact(2)
        .map(|v| i16::from_le_bytes([v[0], v[1]]) as f32 / 32768.0)
        .collect();
    if samples.len() < RATE / 10 {
        return Err("Запис закороткий".into());
    }
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wav_transport_round_trips_and_rejects_truncation() {
        let samples: Vec<_> = (0..16000).map(|i| (i as f32 * 0.03).sin() * 0.5).collect();
        let encoded = wav(&samples);
        let decoded = read_wav(&encoded).unwrap();
        assert_eq!(decoded.len(), samples.len());
        assert!(
            decoded
                .iter()
                .zip(&samples)
                .all(|(a, b)| (a - b).abs() < 0.0001)
        );
        assert!(read_wav(&encoded[..encoded.len() - 1]).is_err());
    }
    #[test]
    fn resampler_preserves_duration_and_rejects_ultrasonic_aliasing() {
        let signal = |hz: f32| {
            (0..48000)
                .map(|i| (std::f32::consts::TAU * hz * i as f32 / 48000.0).sin())
                .collect::<Vec<_>>()
        };
        let low = resample(&signal(1000.0), 48000).unwrap();
        let high = resample(&signal(12000.0), 48000).unwrap();
        assert_eq!(low.len(), 16000);
        let energy = |v: &[f32]| v[1000..15000].iter().map(|v| v * v).sum::<f32>();
        assert!(energy(&low) > 1000.0);
        assert!(energy(&high) < energy(&low) * 0.01);
    }

    #[test]
    fn snapshots_resample_without_mutating_the_raw_buffer() {
        let error = Mutex::new(None);
        for rate in [44_100, 48_000] {
            let raw: Vec<_> = (0..rate)
                .map(|i| (std::f32::consts::TAU * 1_000.0 * i as f32 / rate as f32).sin())
                .collect();
            let samples = Mutex::new(raw.clone());

            let first = snapshot_samples(&samples, &error, rate).unwrap();
            let second = snapshot_samples(&samples, &error, rate).unwrap();

            assert_eq!(first.len(), RATE);
            assert_eq!(first, second);
            assert_eq!(*samples.lock().unwrap(), raw);
        }
    }

    #[test]
    fn snapshot_reports_but_does_not_consume_capture_errors() {
        let error = Mutex::new(Some("capture failed".to_owned()));
        let samples = Mutex::new(vec![0.0; RATE]);

        assert_eq!(
            snapshot_samples(&samples, &error, RATE).unwrap_err(),
            "capture failed"
        );
        assert_eq!(error.lock().unwrap().as_deref(), Some("capture failed"));
    }
}
