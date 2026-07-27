//! Diagnostics.
//!
//! The machine-readable verdict a compiler gates on: a document shaped like an
//! LSP diagnostic list, extended with the `@loc` of each offending object, so an
//! editor's language server can pass it straight through.
//!
//! A compiler reads one field. `status` is `errors` or `ok`; on `errors` it
//! renders the list and stops before codegen, on `ok` it proceeds against the
//! very same XMIR. Everything else in here serves the reader rather than the
//! gate: `code` is the stable string worth matching on, `message` is for a
//! person, and `detail` is so nobody has to parse the message.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::render::{explain, show};
use crate::solver::Clash;
use crate::types::{Site, Types};
use crate::xmir::{Broken, Trouble};

/// The version of this document's shape.
const SCHEMA: &str = "eo-type-diagnostics/1";

/// Whether the build may go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Nothing was rejected. Proceed to codegen against the same XMIR.
    Ok,
    /// Something was rejected. Render the diagnostics and stop.
    Errors,
}

/// How much a finding matters. Only an error gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Stops the build.
    Error,
    /// Worth saying, does not stop the build.
    Warning,
}

/// Where in a file a finding sits, for placing a caret under it.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Range {
    /// The line it was written on.
    pub line: u32,
    /// How far into the line.
    pub pos: u32,
}

/// The counts a reader wants before reading anything else.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Summary {
    /// How many findings gate the build.
    pub errors: usize,
    /// How many do not.
    pub warnings: usize,
    /// How many top-level objects were looked at.
    pub objects: usize,
}

/// One finding.
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    /// Whether it gates.
    pub severity: Severity,
    /// The stable machine string. This is the real API.
    pub code: &'static str,
    /// The offending object, by the locator that survives reformatting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loc: Option<String>,
    /// Where to put the caret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    /// One sentence, for a person.
    pub message: String,
    /// The same facts, structured, so nobody parses the sentence.
    pub detail: BTreeMap<String, String>,
}

/// The whole verdict.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    /// The shape of this document.
    pub schema: &'static str,
    /// What was looked at.
    pub source: String,
    /// The one field a compiler gates on.
    pub status: Status,
    /// The counts.
    pub summary: Summary,
    /// Everything found.
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    /// The document, as the JSON a consumer reads.
    ///
    /// # Panics
    ///
    /// Never in practice: every field of this document serializes.
    #[must_use]
    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).expect("the report cannot be written as JSON")
    }
}

/// What has been found so far.
///
/// It counts objects apart from findings, because the two do not line up: one
/// broken reference is a finding about no object at all, and one rejected object
/// may be the only thing wrong with a file full of good ones.
#[derive(Debug, Default)]
pub struct Findings {
    objects: usize,
    refused: usize,
    flagged: usize,
    diagnostics: Vec<Diagnostic>,
}

impl Findings {
    /// Note an object that typed without complaint.
    pub fn typed(&mut self) {
        self.objects += 1;
    }

    /// Note an object the checker would not type, and whether that stops a
    /// build or merely deserves saying.
    pub fn rejected(&mut self, types: &Types, clash: &Clash) {
        self.objects += 1;
        if clash.gates() {
            self.refused += 1;
        } else {
            self.flagged += 1;
        }
        let site = Self::site(clash);
        self.diagnostics.push(Diagnostic {
            severity: if clash.gates() {
                Severity::Error
            } else {
                Severity::Warning
            },
            code: clash.code(),
            loc: site.and_then(|site| site.loc.clone()),
            range: site.and_then(|site| site.at).map(|at| Range {
                line: at.line,
                pos: at.pos,
            }),
            message: explain(types, clash),
            detail: Self::detail(types, clash),
        });
    }

    /// Note a forma naming an object that is not there.
    pub fn dangling(&mut self, broken: &Broken) {
        let Broken { forma, meant } = broken;
        let mut detail = BTreeMap::from([("forma".to_owned(), forma.path.clone())]);
        if let Some(meant) = meant {
            detail.insert("meant".to_owned(), meant.clone());
        }
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "ref/dangling-forma",
            loc: forma.site.loc.clone(),
            range: forma.site.at.map(|at| Range {
                line: at.line,
                pos: at.pos,
            }),
            message: broken.to_string(),
            detail,
        });
    }

    /// Note an input the checker never got to look at. It gates: a gate that
    /// waves through what it could not read is worse than no gate.
    pub fn unreadable(&mut self, trouble: &Trouble) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "input/unreadable",
            loc: None,
            range: None,
            message: trouble.to_string(),
            detail: BTreeMap::new(),
        });
    }

    /// How many findings gate the build.
    #[must_use]
    pub fn errors(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|found| found.severity == Severity::Error)
            .count()
    }

    /// How many objects the checker would not type, of the ones that stop a
    /// build.
    #[must_use]
    pub fn refused(&self) -> usize {
        self.refused
    }

    /// How many objects were only flagged, which no build stops for.
    #[must_use]
    pub fn flagged(&self) -> usize {
        self.flagged
    }

    /// Everything found, as a document.
    #[must_use]
    pub fn report(self, source: &str) -> Report {
        let errors = self.errors();
        Report {
            schema: SCHEMA,
            source: source.to_owned(),
            status: if errors == 0 {
                Status::Ok
            } else {
                Status::Errors
            },
            summary: Summary {
                errors,
                warnings: self.diagnostics.len() - errors,
                objects: self.objects,
            },
            diagnostics: self.diagnostics,
        }
    }

    /// Where a clash happened, when it knows.
    fn site(clash: &Clash) -> Option<&Site> {
        match clash {
            Clash::UnknownAttribute { site, .. } | Clash::IncompleteDispatch { site, .. } => {
                Some(site)
            }
            Clash::NotRecovered
            | Clash::NoAttributes { .. }
            | Clash::Mismatch { .. }
            | Clash::Unbound { .. } => None,
        }
    }

    /// The facts of a clash, so a consumer need not read the sentence.
    fn detail(types: &Types, clash: &Clash) -> BTreeMap<String, String> {
        match clash {
            Clash::UnknownAttribute {
                attribute,
                receiver,
                ..
            } => BTreeMap::from([
                ("attribute".to_owned(), attribute.clone()),
                ("receiver".to_owned(), show(types, *receiver)),
            ]),
            Clash::NotRecovered => BTreeMap::new(),
            Clash::IncompleteDispatch {
                unset, attribute, ..
            } => BTreeMap::from([
                ("unset".to_owned(), unset.clone()),
                ("attribute".to_owned(), attribute.clone()),
            ]),
            Clash::NoAttributes { value, wanted } => BTreeMap::from([
                ("value".to_owned(), show(types, *value)),
                ("wanted".to_owned(), wanted.join(" ")),
            ]),
            Clash::Mismatch { value, wanted } => BTreeMap::from([
                ("value".to_owned(), show(types, *value)),
                ("wanted".to_owned(), show(types, *wanted)),
            ]),
            Clash::Unbound { name } => BTreeMap::from([("name".to_owned(), name.clone())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Findings, Severity, Status};
    use crate::solver::Clash;
    use crate::types::{Site, Types};
    use crate::xmir::{Broken, Forma, Trouble};

    /// The clash the object-level fragility pass raises, which is a smell rather
    /// than a shape error.
    fn smell() -> Clash {
        Clash::IncompleteDispatch {
            unset: "uri".to_owned(),
            attribute: "separator".to_owned(),
            site: Site::default(),
        }
    }

    #[test]
    fn says_a_clean_run_may_go_on() {
        let mut findings = Findings::default();
        findings.typed();
        assert_eq!(
            findings.report("somewhere").status,
            Status::Ok,
            "a run that rejected nothing dont say the build may go on"
        );
    }

    #[test]
    fn stops_a_build_that_rejected_something() {
        let mut findings = Findings::default();
        findings.rejected(&Types::default(), &Clash::NotRecovered);
        assert_eq!(
            findings.report("somewhere").status,
            Status::Errors,
            "a run that rejected something dont stop the build"
        );
    }

    #[test]
    fn carries_the_stable_code_of_every_finding() {
        let mut findings = Findings::default();
        findings.rejected(&Types::default(), &Clash::NotRecovered);
        assert_eq!(
            findings.report("somewhere").diagnostics[0].code,
            "type/not-recovered",
            "a finding dont carry the machine string a gate reads"
        );
    }

    #[test]
    fn counts_the_objects_it_looked_at() {
        let mut findings = Findings::default();
        findings.typed();
        findings.rejected(&Types::default(), &Clash::NotRecovered);
        assert_eq!(
            findings.report("somewhere").summary.objects,
            2,
            "the summary dont count every object that was looked at"
        );
    }

    #[test]
    fn writes_itself_as_json_a_consumer_can_read() {
        let mut findings = Findings::default();
        findings.typed();
        assert!(
            findings
                .report("app/target/eo/1-parse")
                .json()
                .contains("\"status\": \"ok\""),
            "the report dont write the one field a compiler gates on"
        );
    }

    #[test]
    fn lets_a_build_go_on_over_a_design_smell() {
        let mut findings = Findings::default();
        findings.rejected(&Types::default(), &smell());
        assert_eq!(
            findings.report("somewhere").status,
            Status::Ok,
            "a half-built object dont let the build it only smells to go on"
        );
    }

    #[test]
    fn counts_a_design_smell_among_the_warnings() {
        let mut findings = Findings::default();
        findings.rejected(&Types::default(), &smell());
        assert_eq!(
            findings.report("somewhere").summary.warnings,
            1,
            "a finding that stops nothing dont count as a warning"
        );
    }

    #[test]
    fn says_a_design_smell_is_only_worth_saying() {
        let mut findings = Findings::default();
        findings.rejected(&Types::default(), &smell());
        assert_eq!(
            findings.report("somewhere").diagnostics[0].severity,
            Severity::Warning,
            "a design smell dont carry the severity that spares a build"
        );
    }

    #[test]
    fn keeps_an_object_it_only_flagged_out_of_the_refusals() {
        let mut findings = Findings::default();
        findings.rejected(&Types::default(), &smell());
        assert_eq!(
            findings.refused(),
            0,
            "an object that was only flagged dont stay out of the refusals"
        );
    }

    #[test]
    fn stops_a_build_it_could_not_even_read_the_input_of() {
        let mut findings = Findings::default();
        findings.unreadable(&Trouble::Malformed("foo.xmir: unknown token".to_owned()));
        assert_eq!(
            findings.report("somewhere").status,
            Status::Errors,
            "an input nobody could read dont stop the build that waved it through"
        );
    }

    #[test]
    fn says_what_a_forma_that_dropped_its_root_was_meant_to_name() {
        let mut findings = Findings::default();
        findings.dangling(&Broken {
            forma: Forma {
                path: "Φ.return".to_owned(),
                site: Site::default(),
            },
            meant: Some("Φ.posix.return".to_owned()),
        });
        assert_eq!(
            findings.report("somewhere").diagnostics[0].message,
            "forma `Φ.return` names no object, though `Φ.posix.return` does",
            "a broken forma dont say what it was meant to name"
        );
    }
}
