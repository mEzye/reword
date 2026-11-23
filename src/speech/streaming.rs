//! Bounded, latest-only previews over the offline Ukrainian model.
//! Final jobs are never replaced by preview jobs; completed phrases are cached.
use super::audio::RATE;
use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex},
};

pub struct Job {
    pub id: u64,
    pub target: usize,
    pub samples: Vec<f32>,
    pub finished: bool,
}

#[derive(Default)]
struct Pending {
    finals: VecDeque<Job>,
    preview: Option<Job>,
    closed: bool,
    finished_id: u64,
}

#[derive(Default)]
pub struct Mailbox {
    pending: Mutex<Pending>,
    wake: Condvar,
}

impl Mailbox {
    pub fn publish(&self, job: Job) -> Result<(), ()> {
        let mut pending = self.pending.lock().unwrap();
        if pending.closed {
            return Err(());
        }
        if job.id <= pending.finished_id {
            return Ok(());
        }
        if job.finished {
            if pending.finals.len() >= 4 {
                return Err(());
            }
            if pending
                .preview
                .as_ref()
                .is_some_and(|preview| preview.id <= job.id)
            {
                pending.preview = None;
            }
            pending.finished_id = job.id;
            pending.finals.push_back(job);
        } else {
            pending.preview = Some(job);
        }
        self.wake.notify_one();
        Ok(())
    }

    pub fn can_start(&self) -> bool {
        let pending = self.pending.lock().unwrap();
        !pending.closed && pending.finals.len() < 4
    }

    pub fn take(&self) -> Option<Job> {
        let mut pending = self.pending.lock().unwrap();
        loop {
            if pending.closed {
                return None;
            }
            if let Some(job) = pending
                .finals
                .pop_front()
                .or_else(|| pending.preview.take())
            {
                return Some(job);
            }
            pending = self.wake.wait(pending).unwrap();
        }
    }

    pub fn close(&self) {
        let mut pending = self.pending.lock().unwrap();
        pending.closed = true;
        pending.preview = None;
        pending.finals.clear();
        self.wake.notify_all();
    }
}

#[derive(Default)]
pub struct Decoder {
    id: u64,
    cursor: usize,
    committed: String,
    tentative: String,
    last_end: usize,
}

fn join(left: &str, right: &str) -> String {
    match (left.trim(), right.trim()) {
        ("", right) => right.into(),
        (left, "") => left.into(),
        (left, right) => format!("{left} {right}"),
    }
}

fn voiced(samples: &[f32]) -> bool {
    !samples.is_empty()
        && samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32 > 0.00001
}

// Finish on a 600 ms pause, retaining its silence for phoneme context. A bounded
// window also prevents continuous speech from re-encoding an entire long take.
fn boundary(samples: &[f32]) -> Option<usize> {
    let frame = RATE / 50;
    let mut speech = false;
    let mut quiet = 0;
    for (i, block) in samples.chunks_exact(frame).enumerate() {
        if voiced(block) {
            speech = true;
            quiet = 0;
        } else {
            quiet += 1;
        }
        let end = (i + 1) * frame;
        if speech && quiet >= 30 && end >= RATE {
            return Some(end);
        }
    }
    if samples.len() >= RATE * 8 {
        // Prefer a low-energy word boundary within the final second.
        let start = RATE * 7;
        let offset = samples[start..RATE * 8]
            .chunks_exact(frame)
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let power = |block: &[f32]| block.iter().map(|v| v * v).sum::<f32>();
                power(a).total_cmp(&power(b))
            })
            .map_or(0, |(i, _)| i * frame);
        return Some(start + offset + frame);
    }
    None
}

impl Decoder {
    pub fn update(
        &mut self,
        job: &Job,
        mut recognize: impl FnMut(&[f32]) -> Result<String, String>,
        mut preview: impl FnMut(String, String),
    ) -> Result<String, String> {
        if self.id != job.id {
            *self = Self {
                id: job.id,
                ..Self::default()
            };
        }
        while self.cursor < job.samples.len() {
            let remaining = &job.samples[self.cursor..];
            let end = boundary(remaining);
            let closed = end.is_some() || job.finished;
            let end = self.cursor + end.unwrap_or(remaining.len());
            if end == self.last_end {
                if closed {
                    self.committed = join(&self.committed, &self.tentative);
                    self.tentative.clear();
                    self.cursor = end;
                }
                break;
            }
            let audio = &job.samples[self.cursor..end];
            if audio.len() < RATE / 5 || !voiced(audio) {
                if closed {
                    self.cursor = end;
                    self.tentative.clear();
                    continue;
                }
                break;
            }
            self.tentative = recognize(audio)?;
            self.last_end = end;
            if closed {
                self.committed = join(&self.committed, &self.tentative);
                self.tentative.clear();
                self.cursor = end;
            }
            preview(
                join(&self.committed, &self.tentative),
                self.committed.clone(),
            );
            if !closed {
                break;
            }
        }
        Ok(join(&self.committed, &self.tentative))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(id: u64, finished: bool, seconds: usize) -> Job {
        Job {
            id,
            target: 0,
            samples: vec![0.1; RATE * seconds],
            finished,
        }
    }
    #[test]
    fn previews_coalesce_and_final_replaces_same_take_preview() {
        let mailbox = Mailbox::default();
        mailbox.publish(job(1, false, 1)).unwrap();
        mailbox.publish(job(1, false, 2)).unwrap();
        assert_eq!(mailbox.take().unwrap().samples.len(), RATE * 2);
        mailbox.publish(job(1, false, 3)).unwrap();
        mailbox.publish(job(1, true, 4)).unwrap();
        mailbox.publish(job(2, false, 1)).unwrap();
        assert!(mailbox.take().unwrap().finished);
        assert_eq!(mailbox.take().unwrap().id, 2);
        mailbox.close();
        assert!(mailbox.take().is_none());
    }
    #[test]
    fn old_previews_cannot_follow_a_newer_final_and_queue_rejects_new_recording_when_full() {
        let mailbox = Mailbox::default();
        mailbox.publish(job(1, false, 1)).unwrap();
        mailbox.publish(job(2, true, 1)).unwrap();
        mailbox.publish(job(1, false, 2)).unwrap();
        assert_eq!(mailbox.take().unwrap().id, 2);
        assert!(mailbox.pending.lock().unwrap().preview.is_none());
        for id in 3..7 {
            mailbox.publish(job(id, true, 1)).unwrap();
        }
        assert!(!mailbox.can_start());
        let _ = mailbox.take();
        assert!(mailbox.can_start());
    }
    #[test]
    fn final_flush_reuses_completed_preview_without_second_inference() {
        let mut decoder = Decoder::default();
        let mut calls = 0;
        let mut recognize = |_: &[f32]| {
            calls += 1;
            Ok("Перевірка".into())
        };
        let mut previews = Vec::new();
        decoder
            .update(&job(1, false, 2), &mut recognize, |text, _| {
                previews.push(text)
            })
            .unwrap();
        assert_eq!(previews, ["Перевірка"]);
        let result = decoder
            .update(&job(1, true, 2), &mut recognize, |_, _| {})
            .unwrap();
        assert_eq!(result, "Перевірка");
        assert_eq!(calls, 1);
    }
    #[test]
    fn pauses_commit_phrases_and_new_take_resets_cached_text() {
        let mut decoder = Decoder::default();
        let mut audio = vec![0.1; RATE];
        audio.extend(vec![0.0; RATE]);
        audio.extend(vec![0.1; RATE]);
        let take = Job {
            id: 1,
            target: 0,
            samples: audio,
            finished: true,
        };
        let mut calls = 0;
        let text = decoder
            .update(
                &take,
                |_| {
                    calls += 1;
                    Ok(format!("слово{calls}"))
                },
                |_, _| {},
            )
            .unwrap();
        assert_eq!(text, "слово1 слово2");
        assert_eq!(
            decoder
                .update(&job(2, true, 1), |_| Ok("новий".into()), |_, _| {})
                .unwrap(),
            "новий"
        );
    }
}
