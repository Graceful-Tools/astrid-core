//! Walking a paginated endpoint to the end.
//!
//! Ported from `astrid-ios/Astrid App/Core/Networking/PaginatedFetch.swift`, including the reason
//! it ignores the server's own count.
//!
//! **Why `total` is captured and never used to stop.** Some endpoints — notably the v1 tasks list,
//! whose `where` clause is a complex `OR` over creator, assignee and list membership — run a count
//! query that disagrees with the query that actually returns rows: stale, approximate, or simply
//! planned differently. Trusting `total` silently drops tasks for people with large libraries, and
//! it drops them from the *end*, where nobody notices. So the loop stops on a short page or an
//! empty one, which is a fact about the data it was handed rather than a claim about data it was
//! not.

use std::future::Future;

/// One page as the server returned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// The server's count for the whole result set. Kept for diagnostics; see the module note.
    pub total: Option<i64>,
}

/// Repeatedly call `fetch_page(limit, offset)` until the server runs out of rows.
///
/// Pure apart from the closure: every network call happens inside it, which is what lets the
/// boundary cases — exact-multiple, short last page, empty first page, a lying total — be ordinary
/// unit tests.
pub async fn fetch_all<T, E, F, Fut>(limit: usize, mut fetch_page: F) -> Result<Vec<T>, E>
where
    F: FnMut(usize, usize) -> Fut,
    Fut: Future<Output = Result<Page<T>, E>>,
{
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut all = Vec::new();
    let mut offset = 0;
    loop {
        let page = fetch_page(limit, offset).await?;
        let received = page.items.len();
        all.extend(page.items);
        if received < limit {
            break;
        }
        offset += limit;
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn page(items: &[&str], total: Option<i64>) -> Page<String> {
        Page {
            items: items.iter().map(|s| s.to_string()).collect(),
            total,
        }
    }

    async fn collect(pages: Vec<Page<String>>) -> (Vec<String>, Vec<(usize, usize)>) {
        let pages = RefCell::new(pages.into_iter());
        let calls = RefCell::new(Vec::new());
        let items = fetch_all::<String, (), _, _>(2, |limit, offset| {
            calls.borrow_mut().push((limit, offset));
            let next = pages.borrow_mut().next().unwrap_or(Page {
                items: Vec::new(),
                total: None,
            });
            async move { Ok(next) }
        })
        .await
        .expect("no failure scripted");
        (items, calls.into_inner())
    }

    #[tokio::test]
    async fn a_short_first_page_is_the_whole_answer() {
        let (items, calls) = collect(vec![page(&["a"], Some(1))]).await;
        assert_eq!(items, vec!["a"]);
        assert_eq!(calls, vec![(2, 0)], "a short page must not be followed up");
    }

    /// A full page could be the last one, so it always costs one more request to find out.
    #[tokio::test]
    async fn a_full_page_is_followed_by_one_more() {
        let (items, calls) = collect(vec![page(&["a", "b"], Some(2)), page(&[], Some(2))]).await;
        assert_eq!(items, vec!["a", "b"]);
        assert_eq!(calls, vec![(2, 0), (2, 2)]);
    }

    #[tokio::test]
    async fn it_walks_until_the_last_partial_page() {
        let (items, _) = collect(vec![
            page(&["a", "b"], Some(5)),
            page(&["c", "d"], Some(5)),
            page(&["e"], Some(5)),
        ])
        .await;
        assert_eq!(items, vec!["a", "b", "c", "d", "e"]);
    }

    /// The regression this helper exists for: the count query says 2, the rows say otherwise, and
    /// the tasks past the claimed total are the ones a user would never notice missing.
    #[tokio::test]
    async fn a_server_that_understates_the_total_still_yields_everything() {
        let (items, _) = collect(vec![
            page(&["a", "b"], Some(2)),
            page(&["c", "d"], Some(2)),
            page(&["e"], Some(2)),
        ])
        .await;
        assert_eq!(items.len(), 5);
    }

    #[tokio::test]
    async fn an_empty_first_page_ends_immediately() {
        let (items, calls) = collect(vec![page(&[], Some(0))]).await;
        assert!(items.is_empty());
        assert_eq!(calls.len(), 1);
    }

    /// A limit of zero would loop forever asking for nothing.
    #[tokio::test]
    async fn a_limit_of_zero_fetches_nothing() {
        let result: Result<Vec<String>, ()> = fetch_all(0, |_, _| async {
            panic!("must not be called");
        })
        .await;
        assert!(result.expect("no failure").is_empty());
    }

    #[tokio::test]
    async fn a_failure_mid_walk_is_propagated_rather_than_truncating_the_result() {
        let calls = RefCell::new(0);
        let result: Result<Vec<String>, &str> = fetch_all(2, |_, _| {
            *calls.borrow_mut() += 1;
            let attempt = *calls.borrow();
            async move {
                if attempt == 1 {
                    Ok(page(&["a", "b"], Some(4)))
                } else {
                    Err("the server fell over")
                }
            }
        })
        .await;
        assert_eq!(result, Err("the server fell over"));
    }
}
