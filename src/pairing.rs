//! Groups pages into documents. Normally pages go (1,2), (3,4), ...: a QUITTANCE DE REGLEMENT
//! page followed by its check. When a title page shows up where a check should be, the paper
//! order is broken; the neighbouring pages tell how, and pairing resumes on the next title page.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageKind {
    /// Carries the QUITTANCE DE REGLEMENT title: the name side of a document.
    Title,
    /// Anything else: a check, a blank page, or a name page whose title OCR missed.
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Issue {
    /// The name page is followed directly by the next document's name page.
    CheckMissing { next_title: usize },
    /// The check came before its own name page.
    SidesSwapped,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    /// 0-based page indexes of one document.
    Document {
        name: usize,
        check: Option<usize>,
        issue: Option<Issue>,
    },
    /// A page that belongs to no document: an extra page, or a check whose name page is missing.
    Stray { page: usize },
}

pub fn pair(kinds: &[PageKind]) -> Vec<Unit> {
    let kind = |i: usize| kinds.get(i).copied();
    let mut units = Vec::new();
    let mut i = 0;
    while i < kinds.len() {
        let here = kinds[i];
        let next = kind(i + 1);
        let after = kind(i + 2);
        match (here, next) {
            (PageKind::Title, Some(PageKind::Title)) => {
                units.push(Unit::Document {
                    name: i,
                    check: None,
                    issue: Some(Issue::CheckMissing { next_title: i + 1 }),
                });
                i += 1;
            }
            // A title in the check position: either this check belongs to the title page right
            // after it (sides swapped), or it's a leftover and the title page has its own check.
            (PageKind::Other, Some(PageKind::Title)) => {
                if after == Some(PageKind::Other) {
                    units.push(Unit::Stray { page: i });
                    i += 1;
                } else {
                    units.push(Unit::Document {
                        name: i + 1,
                        check: Some(i),
                        issue: Some(Issue::SidesSwapped),
                    });
                    i += 2;
                }
            }
            // No title in the check position: keep the normal pairing. This also covers name
            // pages whose title OCR could not read, so a bad scan never shifts the documents.
            _ => {
                units.push(Unit::Document {
                    name: i,
                    check: next.map(|_| i + 1),
                    issue: None,
                });
                i += 2;
            }
        }
    }
    units
}

impl Issue {
    /// `name` is the 0-based index of the document's name page.
    pub fn describe(self, name: usize) -> String {
        match self {
            Issue::CheckMissing { next_title } => format!(
                "No check after page {} (page {} is a QUITTANCE). Add the missing check.",
                name + 1,
                next_title + 1
            ),
            Issue::SidesSwapped => format!(
                "Pages {} and {} swapped. Re-paired; fix the paper order.",
                name,
                name + 1
            ),
        }
    }
}

pub fn describe_stray(page: usize) -> String {
    format!(
        "Page {}: extra page or its QUITTANCE is missing. Remove it or add the QUITTANCE.",
        page + 1
    )
}

#[cfg(test)]
mod tests {
    use super::PageKind::{Other as C, Title as Q};
    use super::*;

    fn doc(name: usize, check: Option<usize>, issue: Option<Issue>) -> Unit {
        Unit::Document { name, check, issue }
    }

    #[test]
    fn normal_order_pairs_two_by_two() {
        assert_eq!(
            pair(&[Q, C, Q, C]),
            vec![doc(0, Some(1), None), doc(2, Some(3), None)]
        );
    }

    #[test]
    fn missing_check_keeps_later_documents_aligned() {
        assert_eq!(
            pair(&[Q, C, Q, Q, C]),
            vec![
                doc(0, Some(1), None),
                doc(2, None, Some(Issue::CheckMissing { next_title: 3 })),
                doc(3, Some(4), None),
            ]
        );
    }

    #[test]
    fn swapped_sides_are_re_paired() {
        assert_eq!(
            pair(&[Q, C, C, Q, Q, C]),
            vec![
                doc(0, Some(1), None),
                doc(3, Some(2), Some(Issue::SidesSwapped)),
                doc(4, Some(5), None),
            ]
        );
    }

    #[test]
    fn swapped_sides_at_the_end_of_the_file() {
        assert_eq!(
            pair(&[Q, C, C, Q]),
            vec![
                doc(0, Some(1), None),
                doc(3, Some(2), Some(Issue::SidesSwapped))
            ]
        );
    }

    /// Also the shape of a check whose QUITTANCE page is missing; the stray check is then
    /// identified by its RIB.
    #[test]
    fn extra_page_is_set_aside() {
        assert_eq!(
            pair(&[Q, C, C, Q, C]),
            vec![
                doc(0, Some(1), None),
                Unit::Stray { page: 2 },
                doc(3, Some(4), None),
            ]
        );
    }

    #[test]
    fn unreadable_title_does_not_shift_documents() {
        assert_eq!(
            pair(&[C, C, Q, C]),
            vec![doc(0, Some(1), None), doc(2, Some(3), None)]
        );
    }

    #[test]
    fn odd_page_count_leaves_last_name_page_alone() {
        assert_eq!(
            pair(&[Q, C, Q]),
            vec![doc(0, Some(1), None), doc(2, None, None)]
        );
    }
}
