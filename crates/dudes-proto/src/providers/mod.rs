//! Raw vendor payloads, one module per provider CLI.
//!
//! Every inbound struct here is `#[serde(default)]` and none is `deny_unknown_fields`: vendors
//! add fields weekly, and a payload the daemon has never seen must still parse to the fields it
//! knows. Field names are the vendor's, verbatim, with the source cited on each struct. The
//! adapter in `dudes-providers` turns these into [`Event`](crate::event::Event) and encodes a
//! [`HookReply`](crate::reply::HookReply) back into the vendor's reply shape.

use serde::{Deserialize, Deserializer};

/// Read an inbound vendor field that may arrive as an explicit `null`.
///
/// Container-level `#[serde(default)]` covers a key that is *missing*, not one that is present and
/// `null`. Every inbound vendor field whose type does not accept serde's unit visitor therefore
/// carries `#[serde(deserialize_with = "super::null_as_default")]`, which reads `null` as the
/// field's default, exactly as an absent key does. `Option<..>`, `serde_json::Value` and
/// `#[serde(flatten)]` fields already accept or cannot receive a named `null`, so they are exempt.
///
/// Deliberately narrow: `null` becomes the default and every *other* malformed shape still
/// fails. Swallowing those instead would read a garbled `modelUsage` as zero spend rather than
/// raising `Shape`, which is a worse failure than the lost line this helper prevents.
///
/// Deserialize-side only: serialization is untouched, so the exported schema does not move.
pub(crate) fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

pub mod claude;
pub mod codex;
pub mod codex_app_server;
pub mod pi;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    const VENDOR_SOURCES: [(&str, &str); 4] = [
        ("claude.rs", include_str!("claude.rs")),
        ("codex.rs", include_str!("codex.rs")),
        ("codex_app_server.rs", include_str!("codex_app_server.rs")),
        ("pi.rs", include_str!("pi.rs")),
    ];

    /// The bracket depth of `text`, counting `(` and `[` outside string literals. An attribute
    /// rustfmt has wrapped over several lines is complete when this returns to zero.
    fn bracket_depth(text: &str) -> i32 {
        let mut depth = 0;
        let mut in_string = false;
        let mut escaped = false;
        for c in text.chars() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
            } else {
                match c {
                    '"' => in_string = true,
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth -= 1,
                    _ => {}
                }
            }
        }
        depth
    }

    /// Source lines with a rustfmt-wrapped *attribute* rejoined onto one line. Past `max_width`
    /// rustfmt splits `#[serde(default, rename_all = "camelCase")]` over four lines and leaves
    /// `#[serde(` alone on the first. That shape is rustfmt's own output, so `cargo fmt --check`
    /// is clean on it, and a scan reading one line at a time sees an attribute with no `default`
    /// in it and rejects a struct that carries one.
    fn attribute_lines(source: &str) -> Vec<String> {
        let mut lines = Vec::new();
        let mut pending = String::new();
        for raw in source.lines() {
            let line = raw.trim();
            if pending.is_empty() {
                if !line.starts_with("#[") {
                    lines.push(line.to_owned());
                    continue;
                }
                pending.push_str(line);
            } else {
                pending.push(' ');
                pending.push_str(line);
            }
            if bracket_depth(&pending) <= 0 {
                lines.push(std::mem::take(&mut pending));
            }
        }
        if !pending.is_empty() {
            lines.push(pending);
        }
        lines
    }

    /// The top-level arguments of every `#[serde(..)]` attribute in `attrs`:
    /// `#[serde(default, rename(serialize = "a", deserialize = "b"))]` yields two, not four.
    /// Depth- and quote-aware, so a comma inside a nested list or inside a string literal does
    /// not split an argument.
    fn serde_args(attrs: &str) -> Vec<String> {
        const OPEN: &str = "#[serde(";
        let mut args: Vec<String> = Vec::new();
        let mut rest = attrs;
        while let Some(at) = rest.find(OPEN) {
            let body = &rest[at + OPEN.len()..];
            let mut arg = String::new();
            let mut depth = 0i32;
            let mut in_string = false;
            let mut escaped = false;
            let mut consumed = body.len();
            for (i, c) in body.char_indices() {
                if in_string {
                    arg.push(c);
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        in_string = false;
                    }
                    continue;
                }
                match c {
                    ')' if depth == 0 => {
                        consumed = i + c.len_utf8();
                        break;
                    }
                    ',' if depth == 0 => args.push(std::mem::take(&mut arg)),
                    '"' => {
                        in_string = true;
                        arg.push(c);
                    }
                    '(' | '[' => {
                        depth += 1;
                        arg.push(c);
                    }
                    ')' | ']' => {
                        depth -= 1;
                        arg.push(c);
                    }
                    _ => arg.push(c),
                }
            }
            args.push(arg);
            rest = &body[consumed..];
        }
        args.into_iter()
            .map(|arg| arg.trim().to_owned())
            .filter(|arg| !arg.is_empty())
            .collect()
    }

    /// Whether `attrs` carries a real container-level `#[serde(default)]` — the bare form, or
    /// `#[serde(default = "path")]`, which is equally legitimate. The match is on the argument,
    /// never on the presence of the word: `deserialize_with = "x_default"` and
    /// `rename = "default"` merely *contain* it, and this guard exists to assert the attribute.
    fn has_serde_default(attrs: &str) -> bool {
        serde_args(attrs).iter().any(|arg| {
            arg.strip_prefix("default")
                .is_some_and(|rest| rest.is_empty() || rest.trim_start().starts_with('='))
        })
    }

    /// `line` with any visibility prefix removed: `pub `, `pub(crate) `, `pub(in crate::x) `.
    fn without_visibility(line: &str) -> &str {
        match line.strip_prefix("pub") {
            Some(rest) if rest.starts_with('(') => rest
                .find(')')
                .map_or(line, |close| rest[close + 1..].trim_start()),
            Some(rest) if rest.starts_with(' ') => rest.trim_start(),
            _ => line,
        }
    }

    /// The scan behind [`vendor_structs_default_and_never_deny_unknown_fields`], as a function
    /// so that its own blind spots can be measured. `Err` names the first offending struct.
    fn struct_without_serde_default(source: &str) -> Result<(), String> {
        let mut attrs = String::new();
        for line in attribute_lines(source) {
            let line = line.as_str();
            if line.starts_with("#[") {
                attrs.push_str(line);
                attrs.push(' ');
            } else if line.starts_with("//") {
                // A doc line sits between an item's attributes and the item itself, so it must
                // not clear them.
            } else if without_visibility(line).starts_with("struct ") {
                if !has_serde_default(&attrs) {
                    return Err(format!("`{line}` is not #[serde(default)]"));
                }
                attrs.clear();
            } else {
                attrs.clear();
            }
        }
        Ok(())
    }

    /// The forbidding half of [`vendor_structs_default_and_never_deny_unknown_fields`], as a
    /// function so that it can have a control of its own — which the defaulting half has had
    /// all along in [`the_default_scan_matches_the_attribute_not_the_word`] and this half
    /// has not.
    ///
    /// Deliberately a whole-source match rather than an attribute match, which is the opposite
    /// choice from [`has_serde_default`] and for the opposite reason. There, matching the word
    /// rather than the attribute *admitted* a struct that had no default. Here the word is
    /// forbidden outright, so the loose match errs toward refusing: it still catches the wrapped
    /// form rustfmt emits past `max_width`, where the word sits on its own line and no
    /// `#[serde(...)]` line contains it. A mention in a doc comment fires too, and that is the
    /// direction we want to fail in.
    fn source_denies_unknown_fields(source: &str) -> bool {
        source.contains("deny_unknown_fields")
    }

    /// Invariant: no vendor payload is `deny_unknown_fields`, and every inbound struct defaults
    /// its fields. Checked on the source text so a new struct cannot forget it.
    #[test]
    fn vendor_structs_default_and_never_deny_unknown_fields() {
        for (name, source) in VENDOR_SOURCES {
            assert!(
                !source_denies_unknown_fields(source),
                "{name}: deny_unknown_fields is forbidden on vendor payloads"
            );
            if let Err(offender) = struct_without_serde_default(source) {
                panic!("{name}: {offender}");
            }
        }
    }

    /// The forbidding scan's own control: proof that the assertion above is wired to the real
    /// vendor source and would fire if a payload gained the attribute.
    ///
    /// Without it, "no vendor payload denies unknown fields" rests on a search that has never
    /// been shown to return anything — an empty or misrouted corpus would pass it in silence,
    /// and the guard would read as green for the rest of the project's life. The subject is
    /// `claude.rs`'s own text with the attribute planted on a real struct; the negative arm is
    /// the same text untouched. `assert_ne!` is what proves the plant actually landed, so a
    /// renamed `HookInput` cannot quietly turn this control into a tautology.
    ///
    /// Why it matters more than the symmetry: a vendor payload made strict does not fail here,
    /// it fails in production on the vendor's next release, when a field nobody has seen yet
    /// arrives and the daemon refuses the whole event instead of reading the fields it knows.
    #[test]
    fn the_forbidding_scan_fires_when_a_vendor_payload_gains_the_attribute() {
        let pristine = include_str!("claude.rs");
        assert!(
            !source_denies_unknown_fields(pristine),
            "the negative arm: today's vendor source carries no such attribute"
        );

        let planted = pristine.replacen(
            "pub struct HookInput",
            "#[serde(deny_unknown_fields)]\npub struct HookInput",
            1,
        );
        assert_ne!(
            planted, pristine,
            "the plant landed: `HookInput` is really in the corpus this guard reads"
        );
        assert!(
            source_denies_unknown_fields(&planted),
            "a vendor payload that gained the attribute is caught"
        );
    }

    /// The default scan's own coverage, measured rather than assumed.
    ///
    /// Every `caught` arm is a struct the guard must reject, and the first is the defect this
    /// test was written for: `#[serde(deserialize_with = "x_default")]` merely *contains* the
    /// word and satisfied the earlier substring match. Every `accepted` arm is a legitimate
    /// struct, which is what stops this test from passing on a scan that rejects everything —
    /// `#[serde(default = "path")]` is the trap in that direction, a real `default` a naive
    /// tightening to `default)` would break. The wrapped arms are the ones nobody can opt out
    /// of: rustfmt *emits* that shape past `max_width`, so `cargo fmt --check` cannot close it,
    /// and the earlier scan rejected the legitimate one.
    #[test]
    fn the_default_scan_matches_the_attribute_not_the_word() {
        fn with(attrs: &str) -> String {
            format!("{attrs}\npub struct S {{\n    pub model: String,\n}}\n")
        }

        let caught = [
            (
                "deserialize_with whose path ends in default",
                with("#[serde(deserialize_with = \"x_default\")]"),
            ),
            (
                "the word inside a rename value",
                with("#[serde(rename = \"default\")]"),
            ),
            (
                "a derived Default but no serde default",
                with("#[derive(Clone, Debug, Default)]"),
            ),
            (
                "the word only in a doc comment",
                with("/// Every payload is `#[serde(default)]`."),
            ),
            (
                "a wrapped attribute with no default",
                with(
                    "#[serde(\n    rename_all = \"camelCase\",\n    \
                     deserialize_with = \"x_default\"\n)]",
                ),
            ),
            (
                "restricted visibility",
                "#[derive(Deserialize)]\npub(crate) struct S {\n}\n".to_owned(),
            ),
            (
                "path-restricted visibility",
                "#[derive(Deserialize)]\npub(in crate::providers) struct S {\n}\n".to_owned(),
            ),
            (
                "a field's default does not carry to the next struct",
                "#[serde(default)]\npub struct A {\n    #[serde(default)]\n    pub x: u8,\n}\n\
                 \npub struct B {\n}\n"
                    .to_owned(),
            ),
        ];
        for (shape, source) in &caught {
            assert!(
                struct_without_serde_default(source).is_err(),
                "{shape}: a struct without #[serde(default)] went unnoticed"
            );
        }

        let accepted = [
            ("bare default", with("#[serde(default)]")),
            (
                "default with a path",
                with("#[serde(default = \"S::empty\")]"),
            ),
            (
                "default first among arguments",
                with("#[serde(default, rename_all = \"camelCase\")]"),
            ),
            (
                "default last among arguments",
                with("#[serde(rename_all = \"camelCase\", default)]"),
            ),
            (
                "default beside a nested argument list",
                with("#[serde(default, rename(serialize = \"a\", deserialize = \"b\"))]"),
            ),
            (
                "a wrapped attribute carrying default",
                with("#[serde(\n    default,\n    rename_all = \"camelCase\"\n)]"),
            ),
            (
                "another attribute between serde and the struct",
                with("#[serde(default)]\n#[allow(clippy::struct_field_names)]"),
            ),
            (
                "a doc line between the attribute and the struct",
                with("#[serde(default)]\n/// The payload."),
            ),
            (
                "restricted visibility with default",
                "#[serde(default)]\npub(crate) struct S {\n}\n".to_owned(),
            ),
        ];
        for (shape, source) in &accepted {
            assert_eq!(
                struct_without_serde_default(source),
                Ok(()),
                "{shape}: a correctly annotated struct was reported as an offender"
            );
        }
    }

    /// The only named-field types in an inbound vendor payload that need no null helper.
    /// `Option` and `serde_json::Value` implement null directly; `#[serde(flatten)]` is checked
    /// from the field attributes because no JSON key can name it. Everything else is denied by
    /// default: that is what keeps a new scalar or nested struct from escaping the policy.
    const NULL_TOLERANT_HEADS: &[&str] = &["Option", "Value"];

    /// Payloads the daemon sends to the vendor. They derive `Deserialize` for round-trip tests,
    /// but are not inbound vendor fields and therefore are outside the null-default policy.
    const OUTBOUND_VENDOR_STRUCTS: &[&str] = &["HookOutput", "HookSpecificOutput"];

    /// The head of a type: `std::collections::BTreeMap<String, V>` becomes `BTreeMap`.
    fn head_of(ty: &str) -> &str {
        let base = ty.split('<').next().unwrap_or(ty).trim();
        base.rsplit("::").next().unwrap_or(base).trim()
    }

    /// `type Names = Vec<String>;` declarations, so a field typed through an alias is measured
    /// by what it resolves to rather than by the name it is spelled with.
    fn type_aliases(source: &str) -> BTreeMap<String, String> {
        let mut aliases = BTreeMap::new();
        for line in source.lines() {
            let line = line.trim();
            let decl = line
                .strip_prefix("pub(crate) ")
                .or_else(|| line.strip_prefix("pub "))
                .unwrap_or(line);
            if let Some(rest) = decl.strip_prefix("type ")
                && let Some((name, ty)) = rest.split_once(" = ")
            {
                aliases.insert(
                    head_of(name).to_owned(),
                    ty.trim().trim_end_matches(';').trim().to_owned(),
                );
            }
        }
        aliases
    }

    /// The head of `ty` with aliases followed. The hop cap makes a cyclic alias fail the test
    /// rather than hang it.
    fn resolved_head(ty: &str, aliases: &BTreeMap<String, String>) -> String {
        let mut head = head_of(ty).to_owned();
        for _ in 0..8 {
            match aliases.get(&head) {
                Some(target) => head = head_of(target).to_owned(),
                None => break,
            }
        }
        head
    }

    /// Source lines with a rustfmt-wrapped field rejoined. Past `max_width` rustfmt moves the
    /// type onto its own line and leaves the name line ending in a bare `:`. That shape is
    /// rustfmt's own output, so `cargo fmt --check` is clean on it and a scan reading one line
    /// at a time never sees the field at all.
    fn logical_lines(source: &str) -> Vec<String> {
        let mut lines = Vec::new();
        let mut pending: Option<String> = None;
        for raw in source.lines() {
            let line = raw.trim();
            if let Some(mut name) = pending.take() {
                name.push(' ');
                name.push_str(line);
                lines.push(name);
            } else if line.ends_with(':') && !line.starts_with("//") && !line.starts_with("#[") {
                pending = Some(line.to_owned());
            } else {
                lines.push(line.to_owned());
            }
        }
        lines.extend(pending);
        lines
    }

    /// The scan behind [`vendor_fields_default_an_explicit_null`], as a function so that its own
    /// blind spots can be measured. `Err` names the first offending field.
    fn vendor_field_without_null_default(source: &str) -> Result<(), String> {
        let aliases = type_aliases(source);
        let mut attrs = String::new();
        let mut in_struct = false;
        for line in logical_lines(source) {
            let line = line.as_str();
            if let Some(decl) = line
                .strip_prefix("pub struct ")
                .or_else(|| line.strip_prefix("struct "))
            {
                let name = decl.split_whitespace().next().unwrap_or(decl);
                in_struct = line.ends_with('{') && !OUTBOUND_VENDOR_STRUCTS.contains(&name);
                attrs.clear();
            } else if line == "}" {
                in_struct = false;
                attrs.clear();
            } else if !in_struct || line.starts_with("///") {
                // Outside a struct body, or a doc line: a doc line sits between a field's
                // attributes and the field itself, so it must not clear them.
            } else if line.starts_with("#[") {
                attrs.push_str(line);
            } else if line.is_empty() {
                attrs.clear();
            } else {
                let decl = line
                    .strip_prefix("pub(crate) ")
                    .or_else(|| line.strip_prefix("pub(super) "))
                    .or_else(|| line.strip_prefix("pub "))
                    .unwrap_or(line);
                if let Some((field, ty)) = decl.split_once(": ") {
                    let ty = ty.trim().trim_end_matches(',');
                    let head = resolved_head(ty, &aliases);
                    if !NULL_TOLERANT_HEADS.contains(&head.as_str())
                        && !attrs.contains("flatten")
                        && !attrs.contains("null_as_default")
                    {
                        return Err(format!(
                            "`{field}: {ty}` would reject an explicit null; add \
                             #[serde(deserialize_with = \"super::null_as_default\")]"
                        ));
                    }
                }
                attrs.clear();
            }
        }
        Ok(())
    }

    /// Invariant 4: every inbound vendor field whose type has no `visit_unit` defaults an explicit
    /// `null`, so one nulled scalar, container or nested struct cannot erase a result line and its
    /// spend. `Option<..>` and `serde_json::Value` implement null directly; a
    /// `#[serde(flatten)]` field has no JSON key to null. Checked on source so a new field is
    /// denied by default rather than quietly reopening the class.
    #[test]
    fn vendor_fields_default_an_explicit_null() {
        for (name, source) in VENDOR_SOURCES {
            if let Err(offender) = vendor_field_without_null_default(source) {
                panic!("{name}: {offender}");
            }
        }
    }

    /// The scan's own coverage, measured rather than assumed.
    ///
    /// The `caught` arms cover each class the policy quantifies over, including the scalar and
    /// nested-struct cases a container-only scan cannot see. The wrapped arm is one nobody can
    /// opt out of: rustfmt emits that shape past `max_width`. The `accepted` arms are the exact
    /// exemptions, which stop the scan from passing by rejecting every field.
    #[test]
    fn the_null_policy_scan_covers_every_field_class_and_only_its_exemptions() {
        fn wrap(body: &str) -> String {
            format!("pub struct S {{\n{body}\n}}\n")
        }
        let helper = "    #[serde(deserialize_with = \"super::null_as_default\")]";

        let caught = [
            ("scalar", wrap("    pub subtype: String,")),
            ("boolean", wrap("    pub is_error: bool,")),
            ("number", wrap("    pub input_tokens: u64,")),
            ("nested struct", wrap("    pub message: ApiMessage,")),
            ("container", wrap("    pub tools: Vec<String>,")),
            (
                "alias",
                format!(
                    "type Names = Vec<String>;\n{}",
                    wrap("    pub tools: Names,")
                ),
            ),
            (
                "qualified path",
                wrap("    pub tools: std::collections::BTreeMap<String, String>,"),
            ),
            (
                "rustfmt-wrapped",
                wrap("    pub tools:\n        Vec<String>,"),
            ),
            (
                "restricted visibility",
                wrap("    pub(crate) tools: Vec<String>,"),
            ),
        ];
        for (shape, source) in &caught {
            assert!(
                vendor_field_without_null_default(source).is_err(),
                "{shape}: a field that rejects null went unnoticed"
            );
        }

        let accepted = [
            (
                "helper present",
                wrap(&format!("{helper}\n    pub tools: Vec<String>,")),
            ),
            (
                "helper on a wrapped field",
                wrap(&format!("{helper}\n    pub tools:\n        Vec<String>,")),
            ),
            ("option", wrap("    pub tools: Option<Vec<String>>,")),
            ("value", wrap("    pub raw: serde_json::Value,")),
            (
                "alias to option",
                format!(
                    "type Maybe = Option<Vec<String>>;\n{}",
                    wrap("    pub tools: Maybe,")
                ),
            ),
            (
                "flattened",
                wrap("    #[serde(flatten)]\n    pub extra: serde_json::Map<String, Value>,"),
            ),
            (
                "outbound payload",
                "pub struct HookOutput {\n    pub kind: String,\n}\n".to_owned(),
            ),
        ];
        for (shape, source) in &accepted {
            assert_eq!(
                vendor_field_without_null_default(source),
                Ok(()),
                "{shape}: a correctly spelled field was reported as an offender"
            );
        }
    }
}
