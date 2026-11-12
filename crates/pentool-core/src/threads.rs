//! Row-parallel loops for pixel work. Each row (or column strip) is computed
//! by exactly the arithmetic a single thread would use, so output bytes never
//! depend on the thread count. `PENTOOL_THREADS=N` sets it; 1 runs inline.
use std::cell::Cell;
use std::sync::OnceLock;

/// Below this many elements a loop stays on the calling thread.
const MIN_PARALLEL: usize = 1 << 16;

thread_local! { static OVERRIDE: Cell<Option<usize>> = const { Cell::new(None) }; }

/// Run `f` with pixel loops started on this thread using `threads` threads.
pub fn with_threads<T>(threads: usize, f: impl FnOnce() -> T) -> T {
    let previous = OVERRIDE.with(|o| o.replace(Some(threads.clamp(1, 16))));
    let result = f();
    OVERRIDE.with(|o| o.set(previous));
    result
}

/// Threads for pixel loops: `PENTOOL_THREADS`, else the available cores (at
/// most 16).
pub fn count() -> usize {
    static COUNT: OnceLock<usize> = OnceLock::new();
    if let Some(threads) = OVERRIDE.with(Cell::get) {
        return threads;
    }
    *COUNT.get_or_init(|| {
        std::env::var("PENTOOL_THREADS")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
            .clamp(1, 16)
    })
}

/// How many bands to split `len` elements into.
fn bands(len: usize) -> usize {
    if len < MIN_PARALLEL {
        1
    } else {
        count().min(len / (MIN_PARALLEL / 4)).max(1)
    }
}

/// Call `f(first_row, rows)` on bands of whole `width`-element rows of `data`.
pub fn rows<T: Send>(data: &mut [T], width: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if width == 0 || data.is_empty() {
        return;
    }
    let height = data.len() / width;
    let bands = bands(data.len()).min(height).max(1);
    if bands == 1 {
        return f(0, data);
    }
    let per = height.div_ceil(bands);
    std::thread::scope(|scope| {
        let f = &f;
        for (n, band) in data.chunks_mut(per * width).enumerate() {
            scope.spawn(move || f(n * per, band));
        }
    });
}

/// Call `f(x0, x1)` for column strips covering `0..width` of an image with
/// `len` elements; each call returns its strip as `height` rows of `x1 - x0`
/// elements, and the strips are returned in order.
pub fn columns<T: Send>(
    width: usize,
    len: usize,
    f: impl Fn(usize, usize) -> Vec<T> + Sync,
) -> Vec<(usize, usize, Vec<T>)> {
    let bands = bands(len).min(width).max(1);
    let per = width.div_ceil(bands.max(1)).max(1);
    let strips: Vec<(usize, usize)> = (0..width)
        .step_by(per)
        .map(|x0| (x0, (x0 + per).min(width)))
        .collect();
    if strips.len() <= 1 {
        return strips
            .into_iter()
            .map(|(x0, x1)| (x0, x1, f(x0, x1)))
            .collect();
    }
    std::thread::scope(|scope| {
        let f = &f;
        let handles: Vec<_> = strips
            .into_iter()
            .map(|(x0, x1)| scope.spawn(move || (x0, x1, f(x0, x1))))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|e| std::panic::resume_unwind(e))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    #[test]
    fn blur_and_blend_bytes_do_not_depend_on_the_thread_count() {
        let (w, h) = (613, 419);
        let source = image::RgbaImage::from_raw(w, h, noise((w * h * 4) as usize)).unwrap();
        let backdrop =
            image::RgbaImage::from_raw(w, h, noise((w * h * 4) as usize + 7)[7..].to_vec())
                .unwrap();
        let planes: Vec<[f64; 4]> = source
            .pixels()
            .map(|p| p.0.map(|v| f64::from(v) / 255.0))
            .collect();
        let run = |threads| {
            with_threads(threads, || {
                let blurred = crate::imageops::blur_planes(&planes, w, h, 23.0);
                let mut blended = backdrop.clone();
                crate::blend::over(&mut blended, &source, "normal", "srgb").unwrap();
                let mut multiplied = backdrop.clone();
                crate::blend::over(&mut multiplied, &source, "multiply", "linear").unwrap();
                (
                    blurred
                        .iter()
                        .flatten()
                        .map(|v| v.to_bits())
                        .collect::<Vec<_>>(),
                    blended.into_raw(),
                    multiplied.into_raw(),
                )
            })
        };
        let one = run(1);
        for threads in [2, 3, 7, 16] {
            assert!(run(threads) == one, "{threads} threads changed the output");
        }
    }
}
