// Python slice semantics (PySlice_AdjustIndices), shared by str, list, tuple and pdf.

use crate::error::{Error, Result};

/// Indices selected by `seq[start:stop:step]` on a sequence of length `len`.
pub fn indices(len: usize, start: Option<i64>, stop: Option<i64>, step: Option<i64>) -> Result<Vec<usize>> {
    let (start, stop, step) = adjust(len, start, stop, step)?;
    let mut out = Vec::new();
    let mut i = start;
    if step > 0 {
        while i < stop {
            out.push(i as usize);
            i += step;
        }
    } else {
        while i > stop {
            out.push(i as usize);
            i += step;
        }
    }
    Ok(out)
}

/// Normalized (start, stop, step) exactly as CPython computes them.
pub fn adjust(len: usize, start: Option<i64>, stop: Option<i64>, step: Option<i64>) -> Result<(i64, i64, i64)> {
    let len = len as i64;
    let step = step.unwrap_or(1);
    if step == 0 {
        return Err(Error::new("ValueError: slice step cannot be zero"));
    }
    let clamp = |v: Option<i64>, default: i64| -> i64 {
        match v {
            None => default,
            Some(mut v) => {
                if v < 0 {
                    v += len;
                    if v < 0 {
                        v = if step < 0 { -1 } else { 0 };
                    }
                } else if v >= len {
                    v = if step < 0 { len - 1 } else { len };
                }
                v
            }
        }
    };
    let (dstart, dstop) = if step > 0 { (0, len) } else { (len - 1, -1) };
    Ok((clamp(start, dstart), clamp(stop, dstop), step))
}

/// Resolve a single (possibly negative) index.
pub fn index(len: usize, i: i64, what: &str) -> Result<usize> {
    let j = if i < 0 { i + len as i64 } else { i };
    if j < 0 || j >= len as i64 {
        return Err(Error::new(format!("IndexError: {what} index {i} out of range (length {len})")));
    }
    Ok(j as usize)
}

/// Python's `seq[start:stop:step] = items` on a Vec.
pub fn assign<T: Clone>(seq: &mut Vec<T>, start: Option<i64>, stop: Option<i64>, step: Option<i64>, items: Vec<T>) -> Result<()> {
    let (s, e, st) = adjust(seq.len(), start, stop, step)?;
    if st == 1 {
        let s = s as usize;
        let e = (e.max(s as i64)) as usize;
        seq.splice(s..e, items);
        return Ok(());
    }
    let idx = indices(seq.len(), start, stop, step)?;
    if idx.len() != items.len() {
        return Err(Error::new(format!(
            "ValueError: attempt to assign sequence of size {} to extended slice of size {}",
            items.len(),
            idx.len()
        )));
    }
    for (i, item) in idx.into_iter().zip(items) {
        seq[i] = item;
    }
    Ok(())
}

/// Python's `del seq[start:stop:step]`.
pub fn delete<T>(seq: &mut Vec<T>, start: Option<i64>, stop: Option<i64>, step: Option<i64>) -> Result<()> {
    let idx = indices(seq.len(), start, stop, step)?;
    let drop: std::collections::HashSet<usize> = idx.into_iter().collect();
    let mut k = 0;
    seq.retain(|_| {
        let keep = !drop.contains(&k);
        k += 1;
        keep
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn py(len: usize, a: Option<i64>, b: Option<i64>, c: Option<i64>) -> Vec<usize> {
        indices(len, a, b, c).unwrap()
    }

    // Expected values generated with CPython: list(range(n))[a:b:c]
    #[test]
    fn matches_python() {
        let n = Some;
        assert_eq!(py(10, None, None, None), (0..10).collect::<Vec<_>>());
        assert_eq!(py(10, None, None, n(-1)), vec![9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
        assert_eq!(py(10, n(1), n(-1), n(2)), vec![1, 3, 5, 7]);
        assert_eq!(py(10, n(-3), None, None), vec![7, 8, 9]);
        assert_eq!(py(10, None, n(-3), None), vec![0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(py(10, n(8), n(2), n(-2)), vec![8, 6, 4]);
        assert_eq!(py(10, n(100), None, n(-1)), vec![9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
        assert_eq!(py(10, n(-100), n(100), n(3)), vec![0, 3, 6, 9]);
        assert_eq!(py(10, n(5), n(2), None), Vec::<usize>::new());
        assert_eq!(py(10, None, n(-100), n(-1)), vec![9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
        assert_eq!(py(10, n(-1), n(-11), n(-3)), vec![9, 6, 3, 0]);
        assert_eq!(py(0, None, None, n(-1)), Vec::<usize>::new());
        assert_eq!(py(5, n(2), n(2), None), Vec::<usize>::new());
        assert!(indices(3, None, None, Some(0)).is_err());
    }

    #[test]
    fn slice_assign() {
        let mut v: Vec<i32> = (0..6).collect();
        assign(&mut v, Some(1), Some(3), None, vec![9, 9, 9]).unwrap();
        assert_eq!(v, vec![0, 9, 9, 9, 3, 4, 5]);
        let mut v: Vec<i32> = (0..6).collect();
        assign(&mut v, None, None, Some(2), vec![7, 7, 7]).unwrap();
        assert_eq!(v, vec![7, 1, 7, 3, 7, 5]);
        assert!(assign(&mut v, None, None, Some(2), vec![1]).is_err());
        let mut v: Vec<i32> = (0..6).collect();
        assign(&mut v, Some(4), Some(1), None, vec![8]).unwrap();
        assert_eq!(v, vec![0, 1, 2, 3, 8, 4, 5]);
        let mut v: Vec<i32> = (0..6).collect();
        delete(&mut v, None, None, Some(-2)).unwrap();
        assert_eq!(v, vec![0, 2, 4]);
    }
}
