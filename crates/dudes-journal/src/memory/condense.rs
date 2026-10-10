use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::ops::Bound::{Excluded, Unbounded};
use std::path::{Path, PathBuf};

use dudes_proto::{Entry, Record};
use dudes_types::{AgentId, Lsn, Timestamp, Usage};
use serde::{Deserialize, Serialize};

use super::{MemoryApply, MemoryCorruption, MemoryProjection, SECURE_OUT_OF_LINE_BODY_STORE};

const SUMMARY_BYTES: usize = 8_192;
const RECENT_BYTES: usize = 32_768;
const INPUT_BYTES: usize = 524_288;
const INPUT_SECTIONS: usize = 128;
const EXCERPT_BYTES: usize = 256;
const RECENT_LABEL: &str = "[Recent memory]\n";

/// Startup bounds for deterministic memory maintenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryCondenseConfig {
    threshold: usize,
    keep: usize,
}

impl Default for MemoryCondenseConfig {
    fn default() -> Self {
        Self {
            threshold: 65_536,
            keep: 8,
        }
    }
}

impl MemoryCondenseConfig {
    /// Validate the configured KiB threshold and retained append-section count.
    ///
    /// # Errors
    /// The threshold is outside 1..=1024 KiB or retained sections outside 1..=128.
    pub fn new(after_kib: usize, keep_sections: usize) -> Result<Self, MemoryCondenseError> {
        if !(1..=1_024).contains(&after_kib) || !(1..=128).contains(&keep_sections) {
            return Err(MemoryCondenseError::InvalidConfig);
        }
        Ok(Self {
            threshold: after_kib * 1_024,
            keep: keep_sections,
        })
    }

    /// The complete active-memory threshold in decoded bytes.
    #[must_use]
    pub const fn threshold_bytes(self) -> usize {
        self.threshold
    }

    /// The newest original append sections kept outside a condensed range.
    #[must_use]
    pub const fn keep_sections(self) -> usize {
        self.keep
    }
}

/// A rejected memory-maintenance operation, with no memory text in its diagnostic.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MemoryCondenseError {
    /// A startup value is outside its supported bounds.
    #[error("memory condense configuration is outside its supported bounds")]
    InvalidConfig,
    /// Secure immutable summary traversal is unavailable on this platform.
    #[error("stored memory condense requires secure descriptor-relative traversal")]
    UnsupportedPlatform,
    /// Legacy applied memory contains an unlaunchable NUL byte.
    #[error("legacy applied memory contains U+0000")]
    LegacyNul,
    /// One legacy section cannot fit the finite capture allowance.
    #[error("a memory section exceeds the condense input allowance")]
    InputTooLarge,
    /// A bounded replacement could not preserve one useful exact excerpt and shrink.
    #[error("memory condense produced no useful smaller summary")]
    NoUsefulSummary,
    /// A record, reference, envelope, or predecessor is invalid.
    #[error("invalid memory summary: {0}")]
    InvalidSummary(&'static str),
    /// The existing immutable-store validation rejected the referenced bytes.
    #[error("memory summary integrity failed: {0:?}")]
    Corrupt(MemoryCorruption),
    /// A summary could not be durably prepared or revalidated.
    #[error("memory summary storage failed: {0}")]
    Storage(#[from] io::Error),
}

#[derive(Clone, Debug, Default)]
pub(super) struct RawSize {
    bytes: u64,
    sections: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ActiveSummary {
    pub(super) record_lsn: Lsn,
    #[serde(skip)]
    pub(super) at: Timestamp,
    pub(super) reference: PathBuf,
    pub(super) envelope: Envelope,
    pub(super) rendered: String,
}

impl ActiveSummary {
    pub(super) const fn kind(&self) -> super::MemorySummaryKind {
        match self.envelope.content {
            Content::Exact { .. } => super::MemorySummaryKind::Excerpts,
            Content::Model { .. } => super::MemorySummaryKind::Model,
        }
    }

    /// The entries it covers: how many, and how many bytes.
    pub(super) const fn covered(&self) -> (u64, u64) {
        (self.envelope.covered_sections, self.envelope.covered_bytes)
    }

    pub(super) const fn covered_range(&self) -> (Lsn, Lsn) {
        (self.envelope.covered_from_lsn, self.envelope.to_lsn)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    lsn: Lsn,
    reference: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Excerpt {
    offset: u64,
    text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    lsn: Lsn,
    prefix: Excerpt,
    suffix: Option<Excerpt>,
}

/// The two ways a condensed range's content can be produced.
///
/// `Exact` is a pure function of the source range and predecessor: a replayer recomputes it from
/// the same inputs and must get the same bytes, which is what lets [`MemoryProjection::apply_record`]
/// reject a tampered or buggy envelope byte-for-byte. `Model` is a model's own prose over the same
/// inputs, which no replayer can reproduce — two calls over equal inputs need not agree — so a
/// replayer checks only its bounds and the coverage it claims, never its words. Both still publish
/// through the same journaled, predecessor-checked `MemoryCondense` record, and both are data a
/// launch task carries, never instructions the daemon executes (invariant 3; it is never a `Done`
/// verdict or evidence).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Content {
    /// Deterministic exact source excerpts: the fallback used whenever a model
    /// summary is disabled, refused admission, or fails, times out, or is invalid.
    Exact {
        /// Flat excerpts in increasing source LSN order; see [`extract`].
        pairs: Vec<Pair>,
        /// Covered sections with no surviving excerpt.
        omitted_sections: u64,
    },
    /// A model-written summary of the predecessor's rendered text together with the newly
    /// evicted sections, carrying the gist of the old text instead of verbatim excerpts.
    Model {
        /// The model's own output text, already validated to be non-empty and within
        /// [`SUMMARY_BYTES`] both rendered and encoded.
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Envelope {
    version: u8,
    agent: AgentId,
    from_lsn: Lsn,
    pub(super) to_lsn: Lsn,
    predecessor: Option<Identity>,
    covered_from_lsn: Lsn,
    covered_sections: u64,
    covered_bytes: u64,
    content: Content,
}

/// A finite snapshot of a closed applied range and its expected predecessor.
#[derive(Clone, Debug)]
pub struct CondensePlan {
    agent: AgentId,
    previous: Option<ActiveSummary>,
    sections: Vec<(Lsn, String)>,
}

impl CondensePlan {
    /// The agent this plan covers.
    #[must_use]
    pub const fn agent(&self) -> AgentId {
        self.agent
    }

    /// The previous active summary's own rendered text, when one exists — half of what a model
    /// call re-summarizes, the other half being [`CondensePlan::sections`].
    #[must_use]
    pub fn previous_rendered(&self) -> Option<&str> {
        self.previous
            .as_ref()
            .map(|summary| summary.rendered.as_str())
    }

    /// The newly evicted sections this plan would condense, oldest first, exactly as applied.
    #[must_use]
    pub fn sections(&self) -> &[(Lsn, String)] {
        &self.sections
    }
}

/// An immutable summary whose secure guard stays alive through journal admission.
#[derive(Debug)]
pub struct PreparedMemoryCondense {
    record: Record,
    #[cfg(unix)]
    guard: super::BodyGuard,
}

impl PreparedMemoryCondense {
    /// The existing record shape to append while the preparation stays alive.
    #[must_use]
    pub const fn record(&self) -> &Record {
        &self.record
    }

    /// Revalidate the immutable body handle and its canonical name.
    ///
    /// # Errors
    /// Ownership, link count, name, or exact bytes changed during preparation.
    #[cfg_attr(
        not(unix),
        expect(
            clippy::unused_self,
            reason = "non-Unix retains this guard interface but cannot prepare stored summaries"
        )
    )]
    pub fn revalidate(&self) -> Result<(), MemoryCondenseError> {
        #[cfg(unix)]
        {
            self.guard.validate().map_err(MemoryCondenseError::Storage)
        }
        #[cfg(not(unix))]
        {
            Err(MemoryCondenseError::UnsupportedPlatform)
        }
    }
}

impl MemoryProjection {
    pub(super) fn track_append(&mut self, agent: AgentId, lsn: Lsn, text: &str) {
        let size = self.sizes.entry(agent).or_default();
        size.bytes = size.bytes.saturating_add(as_u64(text.len()));
        size.sections = size.sections.saturating_add(1);
        if let Some(offset) = text.find('\0') {
            let first = self.first_nul.entry(agent).or_insert((lsn, offset));
            if lsn < first.0 {
                *first = (lsn, offset);
            }
        }
    }

    /// Complete active digest bytes without materializing the complete text.
    #[must_use]
    pub fn active_bytes(&self, agent: AgentId) -> u64 {
        let Some(size) = self.sizes.get(&agent) else {
            return 0;
        };
        self.summaries.get(&agent).map_or_else(
            || size.bytes.saturating_add(size.sections.saturating_sub(1)),
            |summary| {
                as_u64(summary.rendered.len())
                    .saturating_add(size.bytes.saturating_sub(summary.envelope.covered_bytes))
                    .saturating_add(
                        size.sections
                            .saturating_sub(summary.envelope.covered_sections),
                    )
            },
        )
    }

    /// Canonical complete active and original history for explicit inspection and replay comparison.
    /// This full export is not used to construct bounded launch work.
    ///
    /// # Errors
    /// The export could not be serialized.
    pub fn export(&self, agent: AgentId) -> Result<Vec<u8>, MemoryCondenseError> {
        #[derive(Serialize)]
        struct Export<'a> {
            version: u8,
            agent: AgentId,
            summary: Option<&'a ActiveSummary>,
            raw: Option<BTreeMap<Lsn, &'a str>>,
        }
        serde_json::to_vec(&Export {
            version: 1,
            agent,
            summary: self.summaries.get(&agent),
            raw: self.entries.get(&agent).map(|entries| {
                entries
                    .iter()
                    .map(|(lsn, applied)| (*lsn, applied.text.as_str()))
                    .collect()
            }),
        })
        .map_err(|_| MemoryCondenseError::InvalidSummary("export serialization failed"))
    }

    /// Whether the complete active history has an evictable prefix above the threshold.
    #[must_use]
    pub fn condense_needed(&self, agent: AgentId, config: MemoryCondenseConfig) -> bool {
        let count = self
            .sizes
            .get(&agent)
            .map_or(0, |size| size.sections)
            .saturating_sub(
                self.summaries
                    .get(&agent)
                    .map_or(0, |s| s.envelope.covered_sections),
            );
        self.active_bytes(agent) > as_u64(config.threshold) && count > as_u64(config.keep)
    }

    /// Whether a durable model-condense admission consumed this agent's current turn allowance.
    /// A refused or unfinished admission counts; only a newer `TurnIssued` renews it.
    #[must_use]
    pub fn model_condense_already_admitted_this_turn(&self, agent: AgentId) -> bool {
        let Some(admitted) = self.last_condense_model_admitted.get(&agent) else {
            return false;
        };
        match self.last_turn_issued.get(&agent) {
            Some(turn) => admitted > turn,
            None => true,
        }
    }

    /// Plain launch recall bounded to 8 KiB of summary and 32 KiB of newest raw tail.
    /// Legacy NUL recall carries exact source bytes with no preceding label.
    #[must_use]
    pub fn work_digest(&self, agent: AgentId) -> String {
        let Some(entries) = self.entries.get(&agent) else {
            return String::new();
        };
        if let Some((lsn, offset)) = self.first_nul.get(&agent) {
            let source = &entries[lsn].text[..=*offset];
            return suffix(source, SUMMARY_BYTES).to_owned();
        }
        let summary = self.summaries.get(&agent);
        let frontier = summary.map_or(Lsn::ZERO, |s| s.envelope.to_lsn);
        let mut selected = Vec::new();
        let mut partial = None;
        let mut used = RECENT_LABEL.len();
        for body in entries
            .range((Excluded(frontier), Unbounded))
            .rev()
            .map(|(_, applied)| &applied.text)
        {
            let separator = usize::from(!selected.is_empty());
            let Some(remaining) = RECENT_BYTES.checked_sub(used + separator) else {
                break;
            };
            if body.len() <= remaining {
                used += separator + body.len();
                selected.push(body.as_str());
            } else {
                if remaining >= '…'.len_utf8() {
                    partial = Some(format!("…{}", suffix(body, remaining - '…'.len_utf8())));
                }
                break;
            }
        }
        let has_recent = selected.iter().any(|s| !s.is_empty()) || partial.is_some();
        let mut result = summary.map_or_else(String::new, |s| s.rendered.clone());
        if has_recent {
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str(RECENT_LABEL);
            let mut wrote = false;
            if let Some(partial) = partial {
                result.push_str(&partial);
                wrote = true;
            }
            for body in selected.into_iter().rev() {
                if wrote {
                    result.push('\n');
                }
                result.push_str(body);
                wrote = true;
            }
        }
        result
    }

    /// Capture at most 128 sections and 512 KiB, excluding the configured recent tail.
    ///
    /// # Errors
    /// Secure storage is unsupported, legacy NUL exists, or one section exceeds the allowance.
    pub fn plan_condense(
        &self,
        agent: AgentId,
        config: MemoryCondenseConfig,
    ) -> Result<Option<CondensePlan>, MemoryCondenseError> {
        if !self.condense_needed(agent, config) {
            return Ok(None);
        }
        if !SECURE_OUT_OF_LINE_BODY_STORE {
            return Err(MemoryCondenseError::UnsupportedPlatform);
        }
        if self.first_nul.contains_key(&agent) {
            return Err(MemoryCondenseError::LegacyNul);
        }
        let entries = &self.entries[&agent];
        let previous = self.summaries.get(&agent).cloned();
        let frontier = previous.as_ref().map_or(Lsn::ZERO, |s| s.envelope.to_lsn);
        let kept = entries
            .keys()
            .rev()
            .nth(config.keep - 1)
            .ok_or(MemoryCondenseError::InvalidSummary("missing retained tail"))?;
        let mut used = previous
            .as_ref()
            .map_or(Ok(0), |s| encode(&s.envelope).map(|bytes| bytes.len()))?;
        let mut sections = Vec::new();
        for (lsn, super::Applied { text: body, .. }) in entries
            .range((Excluded(frontier), Excluded(*kept)))
            .take(INPUT_SECTIONS)
        {
            if used.saturating_add(body.len()) > INPUT_BYTES {
                if sections.is_empty() {
                    return Err(MemoryCondenseError::InputTooLarge);
                }
                break;
            }
            used += body.len();
            sections.push((*lsn, body.clone()));
        }
        Ok((!sections.is_empty()).then_some(CondensePlan {
            agent,
            previous,
            sections,
        }))
    }

    /// Durably prepare a canonical deterministic exact-excerpt summary before its record can be
    /// admitted. The validated fallback used whenever a model summary is disabled, refused
    /// admission, or fails, times out, or returns invalid output.
    ///
    /// # Errors
    /// The plan is stale, its output is empty/non-shrinking, or immutable storage fails.
    pub fn prepare_condense(
        &self,
        plan: CondensePlan,
    ) -> Result<PreparedMemoryCondense, MemoryCondenseError> {
        if !SECURE_OUT_OF_LINE_BODY_STORE {
            return Err(MemoryCondenseError::UnsupportedPlatform);
        }
        self.check_predecessor(plan.agent, plan.previous.as_ref().map(identity).as_ref())?;
        let envelope = build_envelope(&plan)?;
        let agent = plan.agent;
        drop(plan);
        self.finish_prepare(agent, &envelope, None)
    }

    /// Durably prepare a model-written summary of `plan`'s predecessor and evicted sections
    /// before its record can be admitted. `text` is the model's own already
    /// non-empty output; `usage` is what the model call reported it consumed, when it reported
    /// one. `usage: None` is not a claim the call was free: budget folding then prices it like
    /// any other unobserved end, at the operator's configured reserve.
    ///
    /// # Errors
    /// As [`MemoryProjection::prepare_condense`]; additionally when `text` does not shrink the
    /// range it replaces or exceeds the same rendered/encoded bound an exact summary keeps to.
    pub fn prepare_condense_with_model(
        &self,
        plan: CondensePlan,
        text: String,
        usage: Option<Usage>,
    ) -> Result<PreparedMemoryCondense, MemoryCondenseError> {
        if !SECURE_OUT_OF_LINE_BODY_STORE {
            return Err(MemoryCondenseError::UnsupportedPlatform);
        }
        self.check_predecessor(plan.agent, plan.previous.as_ref().map(identity).as_ref())?;
        let envelope = build_model_envelope(&plan, text)?;
        let agent = plan.agent;
        drop(plan);
        self.finish_prepare(agent, &envelope, usage)
    }

    #[cfg_attr(
        not(unix),
        expect(
            clippy::unused_self,
            reason = "non-Unix never reaches this call: both callers return \
                      UnsupportedPlatform first, but the method stays an associated fn so its \
                      shape matches the Unix body that does use self.body_root"
        )
    )]
    fn finish_prepare(
        &self,
        agent: AgentId,
        envelope: &Envelope,
        model_usage: Option<Usage>,
    ) -> Result<PreparedMemoryCondense, MemoryCondenseError> {
        let bytes = encode(envelope)?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let name = format!("{hash}.{}.summary", bytes.len());
        let reference = summary_path(agent, &name);
        #[cfg(unix)]
        let guard = super::persist_immutable_with(
            &self.body_root,
            agent,
            &name,
            &bytes,
            &|_, directory| directory.sync_all(),
        )?;
        Ok(PreparedMemoryCondense {
            record: Record::MemoryCondense {
                agent,
                from_lsn: envelope.from_lsn,
                to_lsn: envelope.to_lsn,
                summary_ref: reference,
                model_usage,
            },
            #[cfg(unix)]
            guard,
        })
    }

    /// Shared live/replay fold that reports invalid condensed records separately from append
    /// corruption. Like [`MemoryProjection::apply`], it labels what it folds with
    /// [`Timestamp::UNIX_EPOCH`].
    ///
    /// # Errors
    /// A condensed record fails secure payload, range, or predecessor validation.
    pub fn apply_record(
        &mut self,
        lsn: Lsn,
        record: &Record,
    ) -> Result<MemoryApply, MemoryCondenseError> {
        self.fold(lsn, Timestamp::UNIX_EPOCH, record)
    }

    /// [`MemoryProjection::apply_record`] for a whole journal entry, keeping the entry's time as
    /// the label of an applied append, a withheld append and an active summary.
    ///
    /// # Errors
    /// As [`MemoryProjection::apply_record`].
    pub fn apply_entry(
        &mut self,
        lsn: Lsn,
        entry: &Entry,
    ) -> Result<MemoryApply, MemoryCondenseError> {
        self.fold(lsn, entry.at, &entry.record)
    }

    pub(super) fn fold(
        &mut self,
        lsn: Lsn,
        at: Timestamp,
        record: &Record,
    ) -> Result<MemoryApply, MemoryCondenseError> {
        if matches!(record, Record::MemoryCondense { .. }) {
            self.fold_condense(lsn, at, record)
        } else {
            Ok(self.fold_plain(lsn, at, record))
        }
    }

    fn fold_condense(
        &mut self,
        lsn: Lsn,
        at: Timestamp,
        record: &Record,
    ) -> Result<MemoryApply, MemoryCondenseError> {
        let Record::MemoryCondense {
            agent,
            from_lsn,
            to_lsn,
            summary_ref,
            model_usage: _,
        } = record
        else {
            return Ok(MemoryApply::Ignored);
        };
        let (name, hash, length) = parse_reference(*agent, summary_ref)?;
        if let Some(current) = self.summaries.get(agent)
            && (lsn <= current.record_lsn
                || (summary_ref == &current.reference
                    && *from_lsn == current.envelope.from_lsn
                    && *to_lsn == current.envelope.to_lsn))
        {
            return Ok(MemoryApply::Ignored);
        }
        if !SECURE_OUT_OF_LINE_BODY_STORE {
            return Err(MemoryCondenseError::UnsupportedPlatform);
        }
        let bytes = super::read_body_relative(&self.body_root, *agent, &name, as_u64(length))
            .map_err(MemoryCondenseError::Corrupt)?;
        if blake3::hash(&bytes).to_hex().as_str() != hash {
            return Err(MemoryCondenseError::Corrupt(
                MemoryCorruption::DigestMismatch,
            ));
        }
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|_| MemoryCondenseError::InvalidSummary("invalid envelope"))?;
        if encode(&envelope)? != bytes {
            return Err(MemoryCondenseError::InvalidSummary("noncanonical envelope"));
        }
        if envelope.version != 1
            || envelope.agent != *agent
            || envelope.from_lsn != *from_lsn
            || envelope.to_lsn != *to_lsn
            || from_lsn > to_lsn
            || *to_lsn >= lsn
        {
            return Err(MemoryCondenseError::InvalidSummary("invalid closed range"));
        }
        self.check_predecessor(*agent, envelope.predecessor.as_ref())?;
        if self.first_nul.contains_key(agent) {
            return Err(MemoryCondenseError::LegacyNul);
        }
        let previous = self.summaries.get(agent).cloned();
        let frontier = previous.as_ref().map_or(Lsn::ZERO, |s| s.envelope.to_lsn);
        let entries = self
            .entries
            .get(agent)
            .ok_or(MemoryCondenseError::InvalidSummary("unknown agent"))?;
        if entries
            .range((Excluded(frontier), Unbounded))
            .next()
            .map(|(lsn, _)| lsn)
            != Some(from_lsn)
            || !entries.contains_key(to_lsn)
        {
            return Err(MemoryCondenseError::InvalidSummary(
                "nonadjacent applied prefix",
            ));
        }
        let mut used = previous
            .as_ref()
            .map_or(Ok(0), |s| encode(&s.envelope).map(|b| b.len()))?;
        let mut sections = Vec::new();
        for (source_lsn, super::Applied { text: body, .. }) in entries.range(*from_lsn..=*to_lsn) {
            if sections.len() >= INPUT_SECTIONS || used.saturating_add(body.len()) > INPUT_BYTES {
                return Err(MemoryCondenseError::InputTooLarge);
            }
            used += body.len();
            sections.push((*source_lsn, body.clone()));
        }
        let plan = CondensePlan {
            agent: *agent,
            previous,
            sections,
        };
        validate_condensed_content(&envelope, &plan)?;
        let rendered = render(&envelope);
        self.summaries.insert(
            *agent,
            ActiveSummary {
                record_lsn: lsn,
                at,
                reference: summary_ref.clone(),
                envelope,
                rendered,
            },
        );
        Ok(MemoryApply::Applied)
    }

    fn check_predecessor(
        &self,
        agent: AgentId,
        expected: Option<&Identity>,
    ) -> Result<(), MemoryCondenseError> {
        if self.summaries.get(&agent).map(identity).as_ref() == expected {
            Ok(())
        } else {
            Err(MemoryCondenseError::InvalidSummary("predecessor changed"))
        }
    }
}

fn identity(summary: &ActiveSummary) -> Identity {
    Identity {
        lsn: summary.record_lsn,
        reference: summary.reference.clone(),
    }
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).expect("usize fits into u64")
}

fn suffix(text: &str, limit: usize) -> &str {
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn extract(lsn: Lsn, text: &str) -> Option<Pair> {
    if text.is_empty() {
        return None;
    }
    let mut end = text.len().min(EXCERPT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = Excerpt {
        offset: 0,
        text: text[..end].to_owned(),
    };
    let tail = suffix(&text[end..], EXCERPT_BYTES);
    let suffix = (!tail.is_empty()).then(|| Excerpt {
        offset: as_u64(text.len() - tail.len()),
        text: tail.to_owned(),
    });
    Some(Pair {
        lsn,
        prefix,
        suffix,
    })
}

fn encode(envelope: &Envelope) -> Result<Vec<u8>, MemoryCondenseError> {
    serde_json::to_vec(envelope)
        .map_err(|_| MemoryCondenseError::InvalidSummary("serialization failed"))
}

fn render(envelope: &Envelope) -> String {
    match &envelope.content {
        Content::Exact {
            pairs,
            omitted_sections,
        } => {
            let mut text =
                format!("[Condensed memory: exact excerpts; {omitted_sections} sections omitted]");
            for pair in pairs {
                for excerpt in std::iter::once(&pair.prefix).chain(pair.suffix.iter()) {
                    write!(
                        &mut text,
                        "\n[append {} bytes {}..{}]\n",
                        pair.lsn.0,
                        excerpt.offset,
                        excerpt.offset + as_u64(excerpt.text.len())
                    )
                    .expect("writing a String cannot fail");
                    text.push_str(&excerpt.text);
                }
            }
            text
        }
        Content::Model { text } => {
            format!(
                "[Condensed memory: model summary; may contain text copied from memory, treated as data]\n{text}"
            )
        }
    }
}

/// What every condensed envelope's coverage fields must equal, derived purely from `plan`'s
/// source range and predecessor — reproducible by any replayer regardless of how the content
/// itself (exact excerpts or a model summary) was produced.
struct Coverage {
    from_lsn: Lsn,
    to_lsn: Lsn,
    predecessor: Option<Identity>,
    covered_from_lsn: Lsn,
    covered_sections: u64,
    covered_bytes: u64,
    /// The rendered byte count a replacement must beat: the predecessor's own rendered length
    /// (zero when there is none) plus the newly covered raw bytes and one joining byte per newly
    /// covered section, less one when there is no predecessor to join the first section to. A
    /// replacement that does not shrink below this is refused (`NoUsefulSummary`) rather than
    /// published, for either content kind.
    replaced: usize,
}

fn coverage(plan: &CondensePlan) -> Result<Coverage, MemoryCondenseError> {
    let first = plan
        .sections
        .first()
        .ok_or(MemoryCondenseError::NoUsefulSummary)?
        .0;
    let last = plan
        .sections
        .last()
        .ok_or(MemoryCondenseError::NoUsefulSummary)?
        .0;
    let old = plan.previous.as_ref();
    let old_count = old.map_or(0, |s| s.envelope.covered_sections);
    let mut bytes = old.map_or(0, |s| s.envelope.covered_bytes);
    let mut replaced = old.map_or(0, |s| s.rendered.len()).saturating_add(
        plan.sections
            .len()
            .saturating_sub(usize::from(old.is_none())),
    );
    for (_, body) in &plan.sections {
        if body.contains('\0') {
            return Err(MemoryCondenseError::LegacyNul);
        }
        bytes = bytes
            .checked_add(as_u64(body.len()))
            .ok_or(MemoryCondenseError::InvalidSummary("byte count overflow"))?;
        replaced = replaced.saturating_add(body.len());
    }
    let covered = old_count.checked_add(as_u64(plan.sections.len())).ok_or(
        MemoryCondenseError::InvalidSummary("section count overflow"),
    )?;
    Ok(Coverage {
        from_lsn: first,
        to_lsn: last,
        predecessor: old.map(identity),
        covered_from_lsn: old.map_or(first, |s| s.envelope.covered_from_lsn),
        covered_sections: covered,
        covered_bytes: bytes,
        replaced,
    })
}

fn build_envelope(plan: &CondensePlan) -> Result<Envelope, MemoryCondenseError> {
    let coverage = coverage(plan)?;
    let old = plan.previous.as_ref();
    let mut candidates: BTreeMap<Lsn, Pair> = old
        .into_iter()
        .flat_map(|s| match &s.envelope.content {
            Content::Exact { pairs, .. } => pairs.clone(),
            Content::Model { .. } => Vec::new(),
        })
        .map(|p| (p.lsn, p))
        .collect();
    for (lsn, body) in &plan.sections {
        if let Some(pair) = extract(*lsn, body) {
            candidates.insert(*lsn, pair);
        }
    }
    let mut envelope = Envelope {
        version: 1,
        agent: plan.agent,
        from_lsn: coverage.from_lsn,
        to_lsn: coverage.to_lsn,
        predecessor: coverage.predecessor.clone(),
        covered_from_lsn: coverage.covered_from_lsn,
        covered_sections: coverage.covered_sections,
        covered_bytes: coverage.covered_bytes,
        content: Content::Exact {
            pairs: Vec::new(),
            omitted_sections: coverage.covered_sections,
        },
    };
    for (_, pair) in candidates.into_iter().rev() {
        let source_lsn = pair.lsn;
        let Content::Exact {
            pairs,
            omitted_sections,
        } = &mut envelope.content
        else {
            unreachable!("the exact builder only ever holds Content::Exact")
        };
        pairs.push(pair);
        pairs.sort_by_key(|p| p.lsn);
        *omitted_sections = coverage.covered_sections - as_u64(pairs.len());
        if encode(&envelope)?.len() > SUMMARY_BYTES || render(&envelope).len() > SUMMARY_BYTES {
            let Content::Exact {
                pairs,
                omitted_sections,
            } = &mut envelope.content
            else {
                unreachable!("the exact builder only ever holds Content::Exact")
            };
            pairs.retain(|pair| pair.lsn != source_lsn);
            *omitted_sections = coverage.covered_sections - as_u64(pairs.len());
        }
    }
    let Content::Exact { pairs, .. } = &envelope.content else {
        unreachable!("the exact builder only ever holds Content::Exact")
    };
    if pairs.is_empty() || render(&envelope).len() >= coverage.replaced {
        return Err(MemoryCondenseError::NoUsefulSummary);
    }
    Ok(envelope)
}

/// Build the envelope for a model-written summary of `plan`'s predecessor and evicted sections.
/// `text` must already be the model's own non-empty output; this only wraps it, bounds it the
/// same way an exact summary is bounded, and requires it to actually shrink what it replaces.
///
/// # Errors
/// `text` is empty, or the rendered or encoded envelope exceeds [`SUMMARY_BYTES`], or it does not
/// shrink below [`Coverage::replaced`].
fn build_model_envelope(
    plan: &CondensePlan,
    text: String,
) -> Result<Envelope, MemoryCondenseError> {
    if text.is_empty() {
        return Err(MemoryCondenseError::NoUsefulSummary);
    }
    let coverage = coverage(plan)?;
    let envelope = Envelope {
        version: 1,
        agent: plan.agent,
        from_lsn: coverage.from_lsn,
        to_lsn: coverage.to_lsn,
        predecessor: coverage.predecessor,
        covered_from_lsn: coverage.covered_from_lsn,
        covered_sections: coverage.covered_sections,
        covered_bytes: coverage.covered_bytes,
        content: Content::Model { text },
    };
    if encode(&envelope)?.len() > SUMMARY_BYTES || render(&envelope).len() > SUMMARY_BYTES {
        return Err(MemoryCondenseError::NoUsefulSummary);
    }
    if render(&envelope).len() >= coverage.replaced {
        return Err(MemoryCondenseError::NoUsefulSummary);
    }
    Ok(envelope)
}

/// [`MemoryProjection::fold_condense`]'s content check, split out only to keep that function's
/// line count down: not reusable on its own, since it assumes the range, predecessor, and
/// adjacency checks the caller already made.
fn validate_condensed_content(
    envelope: &Envelope,
    plan: &CondensePlan,
) -> Result<(), MemoryCondenseError> {
    match &envelope.content {
        // Exact excerpts are a pure function of the source range and predecessor: a replayer
        // recomputes the whole envelope and must get the same bytes back. This is the strong
        // check and it stays exactly as strong as before this record introduced a second
        // content kind.
        Content::Exact { .. } => {
            let expected = build_envelope(plan)?;
            if *envelope != expected {
                return Err(MemoryCondenseError::InvalidSummary(
                    "excerpts or totals differ from source",
                ));
            }
        }
        // A model's prose cannot be recomputed deterministically — two calls over equal inputs
        // need not agree — so a replayer checks only what *is* reproducible from the source
        // alone (the coverage the envelope claims) plus the same bounds a fresh model envelope
        // would have to pass, never the words themselves.
        Content::Model { text } => {
            let expected = coverage(plan)?;
            if envelope.covered_from_lsn != expected.covered_from_lsn
                || envelope.covered_sections != expected.covered_sections
                || envelope.covered_bytes != expected.covered_bytes
            {
                return Err(MemoryCondenseError::InvalidSummary(
                    "model summary coverage differs from source",
                ));
            }
            if text.is_empty() {
                return Err(MemoryCondenseError::InvalidSummary("empty model summary"));
            }
            if encode(envelope)?.len() > SUMMARY_BYTES || render(envelope).len() > SUMMARY_BYTES {
                return Err(MemoryCondenseError::InvalidSummary(
                    "model summary exceeds its bound",
                ));
            }
            if render(envelope).len() >= expected.replaced {
                return Err(MemoryCondenseError::InvalidSummary(
                    "model summary did not shrink the replaced range",
                ));
            }
        }
    }
    Ok(())
}

fn summary_path(agent: AgentId, name: &str) -> PathBuf {
    PathBuf::from("agents")
        .join(agent.to_string())
        .join("memory")
        .join(name)
}

fn parse_reference(
    agent: AgentId,
    path: &Path,
) -> Result<(String, String, usize), MemoryCondenseError> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(MemoryCondenseError::InvalidSummary("invalid path"))?;
    let stem = name
        .strip_suffix(".summary")
        .ok_or(MemoryCondenseError::InvalidSummary("invalid summary name"))?;
    let (hash, length) = stem
        .split_once('.')
        .ok_or(MemoryCondenseError::InvalidSummary("missing length"))?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || length.starts_with('0')
        || !length.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(MemoryCondenseError::InvalidSummary("noncanonical identity"));
    }
    let length: usize = length
        .parse()
        .map_err(|_| MemoryCondenseError::InvalidSummary("invalid length"))?;
    if !(1..=SUMMARY_BYTES).contains(&length)
        || path.as_os_str() != summary_path(agent, name).as_os_str()
    {
        return Err(MemoryCondenseError::InvalidSummary("wrong agent or length"));
    }
    Ok((name.to_owned(), hash.to_owned(), length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_300_has_a_256_prefix_and_44_byte_disjoint_suffix() {
        let pair = extract(Lsn(1), &"a".repeat(300)).unwrap();
        assert_eq!(
            pair.prefix,
            Excerpt {
                offset: 0,
                text: "a".repeat(256)
            }
        );
        assert_eq!(
            pair.suffix,
            Some(Excerpt {
                offset: 256,
                text: "a".repeat(44)
            })
        );
    }

    #[test]
    fn multibyte_excerpts_end_at_exact_source_boundaries() {
        let pair = extract(Lsn(7), &"🦀".repeat(100)).unwrap();
        assert_eq!(
            pair.prefix,
            Excerpt {
                offset: 0,
                text: "🦀".repeat(64)
            }
        );
        assert_eq!(
            pair.suffix,
            Some(Excerpt {
                offset: 256,
                text: "🦀".repeat(36)
            })
        );
    }

    #[test]
    fn actual_json_expansion_bounds_whole_pairs_and_rendered_text() {
        let plan = CondensePlan {
            agent: AgentId::NIL,
            previous: None,
            sections: (1..=128)
                .map(|lsn| (Lsn(lsn), "\u{1}".repeat(4_096)))
                .collect(),
        };
        let envelope = build_envelope(&plan).unwrap();
        assert!(encode(&envelope).unwrap().len() <= SUMMARY_BYTES);
        assert!(render(&envelope).len() <= SUMMARY_BYTES);
        let Content::Exact {
            pairs,
            omitted_sections,
        } = &envelope.content
        else {
            panic!("the exact builder only ever holds Content::Exact")
        };
        assert!(pairs.len() < 128);
        assert_eq!(envelope.covered_sections, 128);
        assert_eq!(*omitted_sections, 128 - as_u64(pairs.len()));
        for pair in pairs {
            assert_eq!(pair.prefix.text.len(), 256);
            assert_eq!(pair.suffix.as_ref().unwrap().text.len(), 256);
        }
    }

    #[test]
    fn full_bound_extraction_probe_keeps_actual_output_bounded() {
        use std::io::Write as _;
        let plan = CondensePlan {
            agent: AgentId::NIL,
            previous: None,
            sections: (1..=128).map(|lsn| (Lsn(lsn), "x".repeat(4_096))).collect(),
        };
        let mut samples = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            let envelope = build_envelope(&plan).unwrap();
            assert_eq!(envelope.covered_bytes, 524_288);
            assert!(encode(&envelope).unwrap().len() <= 8_192);
            assert!(render(&envelope).len() <= 8_192);
            samples.push(start.elapsed().as_micros());
        }
        writeln!(
            std::io::stdout().lock(),
            "{}",
            serde_json::json!({"input_bytes":524_288,"sections":128,"wall_us":samples})
        )
        .unwrap();
    }

    /// A model summary of the same plan is bounded the same way an exact one is, and shares the
    /// same must-shrink refusal — the two content kinds differ in reproducibility, not in bounds.
    #[test]
    fn a_model_summary_is_bounded_like_an_exact_one_and_must_shrink() {
        let plan = CondensePlan {
            agent: AgentId::NIL,
            previous: None,
            sections: vec![(Lsn(1), "decision: use typed argv\ncause: spaces".repeat(50))],
        };
        let short = build_model_envelope(&plan, "uses typed argv because of spaces".to_owned())
            .expect("a short summary that shrinks the range is accepted");
        assert!(matches!(short.content, Content::Model { .. }));
        assert!(render(&short).len() < coverage(&plan).unwrap().replaced);

        let empty = build_model_envelope(&plan, String::new());
        assert!(matches!(empty, Err(MemoryCondenseError::NoUsefulSummary)));

        let oversized = build_model_envelope(&plan, "x".repeat(SUMMARY_BYTES + 1));
        assert!(matches!(
            oversized,
            Err(MemoryCondenseError::NoUsefulSummary)
        ));

        let too_long_to_shrink = build_model_envelope(
            &plan,
            plan.sections[0].1.clone() + &plan.sections[0].1.clone(),
        );
        assert!(matches!(
            too_long_to_shrink,
            Err(MemoryCondenseError::NoUsefulSummary)
        ));
    }
}
