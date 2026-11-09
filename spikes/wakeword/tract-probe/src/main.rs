//! Usage:
//!   tract-probe tflite MODEL.tflite             try to load a microWakeWord model
//!   tract-probe oww MODELS_DIR WAV [WAKEWORD]   openWakeWord pipeline (melspectrogram -> embedding -> classifier)
//!                                               on tract; prints the max score and CPU time per audio second
use std::time::Instant;
use tract_onnx::prelude::*;

type Plan = Arc<TypedRunnableModel>;

fn load_onnx(path: &str, input: Option<&[usize]>) -> TractResult<Plan> {
    let mut m = tract_onnx::onnx().model_for_path(path)?;
    if let Some(shape) = input {
        m = m.with_input_fact(0, f32::fact(shape).into())?;
    }
    m.into_optimized()?.into_runnable()
}

fn main() -> TractResult<()> {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        #[cfg(feature = "tflite")]
        Some("tflite") => {
            match tract_tflite::tflite().model_for_path(&a[2]) {
                Ok(m) => println!("tflite loaded: {} nodes", m.nodes().len()),
                Err(e) => println!("tflite load FAILED: {e:#}"),
            }
            Ok(())
        }
        Some("oww") => oww(&a[2], &a[3], a.get(4).map_or("hey_jarvis_v0.1", |s| s.as_str())),
        _ => {
            eprintln!("usage: see source");
            Ok(())
        }
    }
}

fn oww(dir: &str, wav: &str, word: &str) -> TractResult<()> {
    // The melspectrogram model takes a fixed 1760-sample window per 80 ms step (1280 new + 480 context).
    let t0 = Instant::now();
    let mel = load_onnx(&format!("{dir}/melspectrogram.onnx"), Some(&[1, 1760]))?;
    let emb = load_onnx(&format!("{dir}/embedding_model.onnx"), Some(&[1, 76, 32, 1]))?;
    let cls = load_onnx(&format!("{dir}/{word}.onnx"), Some(&[1, 16, 96]))?;
    eprintln!("models loaded and optimised in {:?}", t0.elapsed());

    let mut r = hound::WavReader::open(wav).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut s: Vec<f32> = r.samples::<i16>().map(|x| x.unwrap() as f32).collect();
    let mut pad = vec![0f32; 16000];
    pad.extend_from_slice(&s);
    pad.extend(std::iter::repeat(0.0).take(16000));
    s = pad;
    let secs = s.len() as f64 / 16000.0;

    let mut mel_buf: Vec<[f32; 32]> = vec![[1.0; 32]; 76];
    let mut feats: Vec<[f32; 96]> = Vec::new();
    let mut best = 0f32;
    let start = Instant::now();
    let mut i = 1760;
    while i <= s.len() {
        let win = &s[i - 1760..i];
        let x = tract_ndarray::Array2::from_shape_vec((1, 1760), win.to_vec())?;
        let out = mel.run(tvec!(x.into_tvalue()))?;
        let v = out[0].to_plain_array_view::<f32>()?;
        // [time, 1, frames, 32] -> frames x 32, scaled as openWakeWord does (x / 10 + 2)
        let frames = v.shape()[2];
        for f in 0..frames {
            let mut row = [0f32; 32];
            for k in 0..32 {
                row[k] = v[[0, 0, f, k]] / 10.0 + 2.0;
            }
            mel_buf.push(row);
        }
        let n = mel_buf.len();
        let flat: Vec<f32> = mel_buf[n - 76..].iter().flat_map(|r| r.iter().copied()).collect();
        let xe = tract_ndarray::Array4::from_shape_vec((1, 76, 32, 1), flat)?;
        let e = emb.run(tvec!(xe.into_tvalue()))?;
        let ev = e[0].to_plain_array_view::<f32>()?;
        let mut fe = [0f32; 96];
        for (k, d) in ev.iter().enumerate().take(96) {
            fe[k] = *d;
        }
        feats.push(fe);
        if mel_buf.len() > 970 {
            mel_buf.drain(0..mel_buf.len() - 970);
        }
        if feats.len() >= 16 {
            let flat: Vec<f32> = feats[feats.len() - 16..].iter().flat_map(|r| r.iter().copied()).collect();
            let xc = tract_ndarray::Array3::from_shape_vec((1, 16, 96), flat)?;
            let o = cls.run(tvec!(xc.into_tvalue()))?;
            best = best.max(o[0].to_plain_array_view::<f32>()?.iter().copied().next().unwrap_or(0.0));
        }
        i += 1280;
    }
    let el = start.elapsed().as_secs_f64();
    if let Ok(st) = std::fs::read_to_string("/proc/self/status") {
        for l in st.lines().filter(|l| l.starts_with("VmHWM") || l.starts_with("VmRSS")) {
            println!("{l}");
        }
    }
    println!("{wav}: max score {best:.3}; {secs:.1} s audio in {el:.3} s -> {:.1}% of one core (wall, single thread)", el / secs * 100.0);
    Ok(())
}
