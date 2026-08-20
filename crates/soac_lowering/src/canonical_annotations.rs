//! Value-only annotation strings from the native parse of this exact source.
//!
//! This data does not authenticate source or grant a runtime capability. The
//! strict runtime owns the native root independently and must consume that same
//! root when it admits the lowered functions.

use std::collections::BTreeMap;

use ruff_text_size::TextRange;
use soac_contracts::{Fingerprint, SourceRange};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalAnnotationStrings {
    source_digest: Fingerprint,
    strings: BTreeMap<SourceRange, String>,
}

impl CanonicalAnnotationStrings {
    /// Assemble semantic data after the caller obtains it from the native AST.
    /// Source binding and range checks do not turn these strings into authority.
    pub fn from_native_entries(
        source: &str,
        entries: impl IntoIterator<Item = (SourceRange, String)>,
    ) -> anyhow::Result<Self> {
        let mut strings = BTreeMap::new();
        for (range, value) in entries {
            anyhow::ensure!(
                range.start < range.end
                    && range.end as usize <= source.len()
                    && source.is_char_boundary(range.start as usize)
                    && source.is_char_boundary(range.end as usize),
                "native annotation string has an invalid source range {range:?}",
            );
            anyhow::ensure!(
                strings.insert(range, value).is_none(),
                "native annotation string has a duplicate source range {range:?}",
            );
        }
        Ok(Self {
            source_digest: Fingerprint::digest(source.as_bytes()),
            strings,
        })
    }

    pub(crate) fn matches_source(&self, source: &str) -> bool {
        self.source_digest == Fingerprint::digest(source.as_bytes())
    }

    pub(crate) fn get(&self, range: TextRange) -> Option<&str> {
        self.strings
            .get(&SourceRange::new(
                range.start().to_u32(),
                range.end().to_u32(),
            ))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_strings_are_source_bound_and_reject_ambiguous_coordinates() {
        let source = "value: tuple['é', int]";
        let range = SourceRange::new(7, source.len() as u32);
        let strings = CanonicalAnnotationStrings::from_native_entries(
            source,
            [(range, "tuple['é', int]".into())],
        )
        .unwrap();
        assert!(strings.matches_source(source));
        assert!(!strings.matches_source("value: tuple['a', int]"));
        assert_eq!(
            strings.get(TextRange::new(range.start.into(), range.end.into())),
            Some("tuple['é', int]"),
        );
        assert!(CanonicalAnnotationStrings::from_native_entries(
            source,
            [(range, "first".into()), (range, "second".into())],
        )
        .is_err());
        let inside_utf8 = source.find('é').unwrap() as u32 + 1;
        assert!(CanonicalAnnotationStrings::from_native_entries(
            source,
            [(SourceRange::new(inside_utf8, range.end), "invalid".into())],
        )
        .is_err());
    }
}
