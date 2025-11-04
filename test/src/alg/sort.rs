/// In-place quicksort with a custom comparator.
pub fn quicksort_by<T, F: FnMut(&T, &T) -> core::cmp::Ordering>(slice: &mut [T], mut cmp: F) {
    fn sort_impl<T, F: FnMut(&T, &T) -> core::cmp::Ordering>(
        v: &mut [T],
        cmp: &mut F,
    ) {
        let len = v.len();
        if len <= 1 { return; }
        let pivot_index = len / 2;
        v.swap(pivot_index, len - 1);
        let mut store = 0;
        for i in 0..len - 1 {
            if cmp(&v[i], &v[len - 1]) == core::cmp::Ordering::Less {
                v.swap(i, store);
                store += 1;
            }
        }
        v.swap(store, len - 1);
        let (left, right) = v.split_at_mut(store);
        // right includes pivot at index 0
        let (_, right_tail) = right.split_first_mut().unwrap();
        sort_impl(left, cmp);
        sort_impl(right_tail, cmp);
    }
    sort_impl(slice, &mut cmp);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quicksort_basic() {
        let mut v = vec![3, 1, 4, 1, 5, 9];
        quicksort_by(&mut v, |a, b| a.cmp(b));
        assert_eq!(v, vec![1, 1, 3, 4, 5, 9]);
    }
}


