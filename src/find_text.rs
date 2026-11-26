use windows_sys::Win32::Globalization::{FIND_FROMSTART, FindStringOrdinal};

use crate::rich_edit::CharRange;

#[derive(Default)]
pub struct Matches {
    starts: Vec<i32>,
    size: i32,
}

impl Matches {
    pub fn new(text: &[u16], query: &str, case_sensitive: bool) -> Self {
        if query.is_empty() {
            return Self::default();
        }
        let needle: Vec<_> = query.encode_utf16().collect();
        let mut starts = Vec::new();
        let mut at = 0;
        while at + needle.len() <= text.len() {
            let rest = &text[at..];
            let index = unsafe {
                FindStringOrdinal(
                    FIND_FROMSTART,
                    rest.as_ptr(),
                    rest.len() as i32,
                    needle.as_ptr(),
                    needle.len() as i32,
                    i32::from(!case_sensitive),
                )
            };
            if index < 0 {
                break;
            }
            at += index as usize;
            starts.push(at as i32);
            at += needle.len();
        }
        Self {
            starts,
            size: needle.len() as i32,
        }
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }
    pub fn empty(&self) -> bool {
        self.starts.is_empty()
    }

    pub fn range(&self, index: usize) -> Option<CharRange> {
        self.starts.get(index).map(|&start| CharRange {
            start,
            end: start + self.size,
        })
    }

    pub fn index(&self, range: CharRange) -> Option<usize> {
        (range.end - range.start == self.size)
            .then(|| self.starts.binary_search(&range.start).ok())
            .flatten()
    }

    pub fn next(&self, cursor: CharRange, backwards: bool, wrap: bool) -> Option<usize> {
        if self.empty() {
            return None;
        }
        if backwards {
            let index = self.starts.partition_point(|&start| start < cursor.start);
            index
                .checked_sub(1)
                .or_else(|| wrap.then_some(self.len() - 1))
        } else {
            let index = self.starts.partition_point(|&start| start < cursor.end);
            if index < self.len() {
                Some(index)
            } else {
                wrap.then_some(0)
            }
        }
    }

    pub fn replace_all(&self, text: &[u16], replacement: &str, limit: usize) -> Option<Vec<u16>> {
        let replacement: Vec<_> = replacement.encode_utf16().collect();
        let size = text.len().checked_sub(self.len() * self.size as usize)?;
        let size = size.checked_add(self.len().checked_mul(replacement.len())?)?;
        if size > limit {
            return None;
        }
        let mut output = Vec::with_capacity(size + 1);
        let mut at = 0;
        for &start in &self.starts {
            output.extend_from_slice(text.get(at..start as usize)?);
            output.extend_from_slice(&replacement);
            at = (start + self.size) as usize;
        }
        output.extend_from_slice(text.get(at..)?);
        output.push(0);
        Some(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_and_replace_use_native_utf16_ranges_and_respect_case_and_wrap() {
        let source: Vec<_> = "🦀 Яма яма".encode_utf16().collect();
        let results = Matches::new(&source, "яма", false);
        assert_eq!(results.len(), 2);
        let first = results.range(0).unwrap();
        let last = results.range(1).unwrap();
        assert_eq!((first.start, first.end), (3, 6));
        assert_eq!(results.next(last, false, false), None);
        assert_eq!(results.next(last, false, true), Some(0));
        assert_eq!(results.next(first, true, true), Some(1));
        assert_eq!(Matches::new(&source, "яма", true).len(), 1);
        assert!(results.replace_all(&source, "test", 4).is_none());
        let replaced = results.replace_all(&source, "!", 100).unwrap();
        assert_eq!(
            String::from_utf16_lossy(&replaced[..replaced.len() - 1]),
            "🦀 ! !"
        );
    }
}
