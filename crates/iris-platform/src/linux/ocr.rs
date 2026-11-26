use crate::{prepare_display_info_gray_3x, DisplayInfoOcrReader};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;

struct OcrJob {
    bgr: Vec<u8>,
    width: usize,
    height: usize,
}

/// Non-blocking Linux OCR reader for `r_DisplayInfo` using `tesseract` CLI if available.
pub struct LinuxDisplayInfoOcr {
    job_tx: Sender<OcrJob>,
    res_rx: Receiver<String>,
}

impl LinuxDisplayInfoOcr {
    pub fn new() -> Self {
        let (job_tx, job_rx) = bounded::<OcrJob>(1);
        let (res_tx, res_rx) = bounded::<String>(4);

        let _ = thread::Builder::new()
            .name("sc-overlay-displayinfo-ocr".to_string())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let (gray_3x, w3, h3) =
                        prepare_display_info_gray_3x(&job.bgr, job.width, job.height);
                    if w3 == 0 || h3 == 0 {
                        continue;
                    }
                    if let Some(text) = run_tesseract_pgm(&gray_3x, w3, h3) {
                        if !text.trim().is_empty() {
                            let _ = res_tx.try_send(text);
                        }
                    }
                }
            });

        Self { job_tx, res_rx }
    }
}

impl Default for LinuxDisplayInfoOcr {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayInfoOcrReader for LinuxDisplayInfoOcr {
    fn submit_top_right_bgr(&self, bgr: Vec<u8>, width: usize, height: usize) {
        let _ = self.job_tx.try_send(OcrJob { bgr, width, height });
    }

    fn try_recv_text(&self) -> Option<String> {
        let mut latest = None;
        while let Ok(t) = self.res_rx.try_recv() {
            latest = Some(t);
        }
        latest
    }
}

fn run_tesseract_pgm(gray: &[u8], w: usize, h: usize) -> Option<String> {
    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout", "--psm", "6", "-l", "eng"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let header = format!("P5\n{} {}\n255\n", w, h);
        stdin.write_all(header.as_bytes()).ok()?;
        stdin.write_all(gray).ok()?;
    }
    let out = child.wait_with_output().ok()?;
    if out.status.success() {
        String::from_utf8(out.stdout).ok()
    } else {
        None
    }
}

