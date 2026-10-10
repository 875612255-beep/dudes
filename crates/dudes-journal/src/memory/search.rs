//! Bounded literal search over folded raw notes and the current validated summary.

use std::ops::Bound::{Excluded, Unbounded};

use dudes_proto::rpc::memory::{MemorySearchHit, MemorySearchOptions, MemorySearchResult};
use dudes_types::{AgentId, Lsn};

use super::{MemoryProjection, condense::ActiveSummary};

const MAX_CANDIDATES: usize = 256;
const MAX_SCAN_BYTES: usize = 1_048_576;
const MAX_RESULT_BYTES: usize = 65_536;
const MAX_EXCERPT_BYTES: usize = 1_024;
const MAX_PRECEDING_BYTES: usize = 256;

fn fits_result(result: &MemorySearchResult) -> Result<bool, MemorySearchError> {
    Ok(serde_json::to_vec(result)
        .map_err(MemorySearchError::Serialization)?
        .len()
        <= MAX_RESULT_BYTES)
}

/// Why a bounded memory search cannot return a page.
#[derive(Debug, thiserror::Error)]
pub enum MemorySearchError {
    /// The first eligible legacy candidate cannot fit the examined-text allowance.
    #[error("memory.search: entry at {lsn} has {bytes} bytes, exceeding the scan bound")]
    EntryTooLarge {
        /// Position that an operator can explicitly skip.
        lsn: Lsn,
        /// Complete candidate byte length.
        bytes: usize,
    },
    /// The typed result could not be serialized for byte accounting.
    #[error("memory.search: result serialization failed")]
    Serialization(#[source] serde_json::Error),
}

enum Candidate<'a> {
    Raw { lsn: Lsn, text: &'a str },
    Summary(&'a ActiveSummary),
}

impl<'a> Candidate<'a> {
    fn lsn(&self) -> Lsn {
        match self {
            Self::Raw { lsn, .. } => *lsn,
            Self::Summary(summary) => summary.record_lsn,
        }
    }

    fn text(&self) -> &'a str {
        match self {
            Self::Raw { text, .. } => text,
            Self::Summary(summary) => &summary.rendered,
        }
    }

    fn hit(self, match_offset: usize, query_bytes: usize) -> MemorySearchHit {
        match self {
            Self::Raw { lsn, text } => {
                let preceding = MAX_PRECEDING_BYTES.min(MAX_EXCERPT_BYTES - query_bytes);
                let mut start = match_offset.saturating_sub(preceding);
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                let mut end = text.len().min(start + MAX_EXCERPT_BYTES);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                MemorySearchHit::Raw {
                    lsn,
                    match_offset,
                    text: text[start..end].to_owned(),
                    text_offset: start,
                    source_bytes: text.len(),
                    truncated: start != 0 || end != text.len(),
                    read_since_lsn: Lsn(lsn.0.saturating_sub(1)),
                }
            }
            Self::Summary(summary) => {
                let (covered_from_lsn, covered_to_lsn) = summary.covered_range();
                MemorySearchHit::Summary {
                    lsn: summary.record_lsn,
                    match_offset,
                    text: summary.rendered.clone(),
                    text_offset: 0,
                    source_bytes: summary.rendered.len(),
                    truncated: false,
                    read_since_lsn: covered_from_lsn.0.checked_sub(1).map(Lsn),
                    covered_from_lsn,
                    covered_to_lsn,
                }
            }
        }
    }
}

impl MemoryProjection {
    /// Search valid raw notes and the current summary in journal order at this folded state.
    /// A page examines at most 256 candidates and 1 MiB of decoded candidate text.
    ///
    /// # Errors
    /// Refuses a first candidate above the scan allowance without consuming it, or reports
    /// a result serialization failure.
    pub fn search(
        &self,
        agent: AgentId,
        options: &MemorySearchOptions,
    ) -> Result<MemorySearchResult, MemorySearchError> {
        let start = options.since_lsn().map_or(Unbounded, Excluded);
        let mut raw = self
            .entries
            .get(&agent)
            .into_iter()
            .flat_map(|entries| entries.range((start, Unbounded)))
            .peekable();
        let mut summary = self.summaries.get(&agent).filter(|summary| {
            options
                .since_lsn()
                .is_none_or(|since| summary.record_lsn > since)
        });
        let mut result = MemorySearchResult::default();
        let mut consumed = None;
        let mut examined = 0;
        let mut examined_bytes = 0;
        loop {
            let has_more = raw.peek().is_some() || summary.is_some();
            if !has_more || result.hits.len() == options.limit() || examined == MAX_CANDIDATES {
                result.next_since_lsn = if has_more { consumed } else { None };
                return Ok(result);
            }
            let candidate = if summary
                .is_some_and(|summary| raw.peek().is_none_or(|(lsn, _)| summary.record_lsn < **lsn))
            {
                Candidate::Summary(summary.take().expect("eligible summary"))
            } else {
                let (lsn, entry) = raw.next().expect("eligible raw entry");
                Candidate::Raw {
                    lsn: *lsn,
                    text: &entry.text,
                }
            };
            let lsn = candidate.lsn();
            let has_more = raw.peek().is_some() || summary.is_some();
            result.next_since_lsn = has_more.then_some(lsn);
            if !fits_result(&result)? {
                result.next_since_lsn = consumed;
                return Ok(result);
            }
            let bytes = candidate.text().len();
            if bytes > MAX_SCAN_BYTES - examined_bytes {
                if examined == 0 {
                    return Err(MemorySearchError::EntryTooLarge { lsn, bytes });
                }
                result.next_since_lsn = consumed;
                return Ok(result);
            }
            examined += 1;
            examined_bytes += bytes;
            if let Some(offset) = candidate.text().find(options.query()) {
                result
                    .hits
                    .push(candidate.hit(offset, options.query().len()));
                if !fits_result(&result)? {
                    result.hits.pop();
                    result.next_since_lsn = consumed;
                    return Ok(result);
                }
            }
            consumed = Some(lsn);
        }
    }
}
