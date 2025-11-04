use std::ops::{Add, Mul};

/// Trait bound grouping for numeric-like types that support addition and multiplication.
pub trait Numeric:
    Copy + Clone + Add<Output = Self> + Mul<Output = Self> + Default
{
}

impl<T> Numeric for T where T: Copy + Clone + Add<Output = T> + Mul<Output = T> + Default {}

/// Compute the dot product of two slices. Returns None if their lengths differ.
pub fn dot<T: Numeric>(left: &[T], right: &[T]) -> Option<T> {
    if left.len() != right.len() {
        return None;
    }
    let mut acc = T::default();
    for (a, b) in left.iter().copied().zip(right.iter().copied()) {
        acc = acc + a * b;
    }
    Some(acc)
}

/// Fixed-size matrix multiplication using const generics (C = A x B).
pub fn matmul<T: Numeric, const R: usize, const C: usize, const K: usize>(
    a: [[T; K]; R],
    b: [[T; C]; K],
) -> [[T; C]; R] {
    let mut out = [[T::default(); C]; R];
    for r in 0..R {
        for c in 0..C {
            let mut sum = T::default();
            for k in 0..K {
                sum = sum + a[r][k] * b[k][c];
            }
            out[r][c] = sum;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_works() {
        let a = [1i32, 2, 3];
        let b = [4i32, 5, 6];
        assert_eq!(dot(&a, &b), Some(32));
    }

    #[test]
    fn matmul_works() {
        let a = [[1i32, 2], [3, 4]];
        let b = [[5i32, 6], [7, 8]];
        let c = matmul::<i32, 2, 2, 2>(a, b);
        assert_eq!(c, [[19, 22], [43, 50]]);
    }
}


