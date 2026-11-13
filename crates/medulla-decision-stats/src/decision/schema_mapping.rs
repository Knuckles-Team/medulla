//! Column-to-property scoring for schema mapping (MEDULLA-DECISION-ENGINE-R033.2.1).
//!
//! A source column maps to an ontology property as a proposal. The score is
//! `w_name x token Jaccard of the names + w_type x datatype compatibility +
//! w_sample x the fraction of sample values that validate`, in integer Q32
//! arithmetic, clamped to `[0, 1]`. The evidence class says how the score is
//! backed: `Proof` for an exact local-name match with a matching SHACL datatype,
//! `Observation` when every sample validates against the property datatype,
//! and `Claim` for anything else. Ties break on the property IRI, ascending.
//! The weights arrive as arguments from the pinned decision policy.

use std::collections::BTreeSet;

use medulla_decision_types::decision::PremiseClass;

/// One in Q32.
pub const ONE: i64 = 1 << 32;
/// Half in Q32: the credit of a lossless but not exact datatype conversion.
const HALF: i64 = ONE / 2;

/// The closed set of value types a SQL column or an XSD datatype maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Text,
    Integer,
    Decimal,
    Boolean,
    Date,
    DateTime,
}

impl ValueType {
    /// The value type of a SQL type name (`varchar(255)`, `BIGINT`,
    /// `double precision`, ...). `None` for a type outside the closed table.
    pub fn from_sql(sql_type: &str) -> Option<Self> {
        let lowered = sql_type.trim().to_ascii_lowercase();
        let stem = lowered.split('(').next().unwrap_or("").trim();
        let first = stem.split_whitespace().next().unwrap_or("");
        Some(match first {
            "smallint" | "tinyint" | "mediumint" | "int" | "integer" | "bigint" | "serial"
            | "bigserial" | "int2" | "int4" | "int8" => Self::Integer,
            "decimal" | "numeric" | "float" | "double" | "real" | "money" | "float4" | "float8" => {
                Self::Decimal
            }
            "boolean" | "bool" | "bit" => Self::Boolean,
            "varchar" | "nvarchar" | "char" | "nchar" | "text" | "string" | "clob" | "uuid"
            | "character" => Self::Text,
            "date" => Self::Date,
            "timestamp" | "datetime" | "datetime2" | "timestamptz" => Self::DateTime,
            _ => return None,
        })
    }

    /// The value type of an XSD datatype IRI or CURIE (`xsd:integer`,
    /// `http://www.w3.org/2001/XMLSchema#dateTime`). `None` outside the table.
    pub fn from_xsd(datatype: &str) -> Option<Self> {
        let local = datatype
            .rsplit(['#', '/', ':'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        Some(match local.as_str() {
            "string" | "normalizedstring" | "token" | "anyuri" => Self::Text,
            "integer" | "int" | "long" | "short" | "byte" | "nonnegativeinteger"
            | "positiveinteger" | "negativeinteger" | "nonpositiveinteger" | "unsignedint"
            | "unsignedlong" => Self::Integer,
            "decimal" | "double" | "float" => Self::Decimal,
            "boolean" => Self::Boolean,
            "date" => Self::Date,
            "datetime" | "datetimestamp" => Self::DateTime,
            _ => return None,
        })
    }

    /// Whether `sample` is a valid lexical form of the type.
    pub fn validates(self, sample: &str) -> bool {
        let sample = sample.trim();
        match self {
            Self::Text => true,
            Self::Integer => sample.parse::<i64>().is_ok(),
            Self::Decimal => sample.parse::<f64>().is_ok_and(f64::is_finite),
            Self::Boolean => matches!(
                sample.to_ascii_lowercase().as_str(),
                "true" | "false" | "0" | "1"
            ),
            Self::Date => valid_date(sample),
            Self::DateTime => match sample.split_once(['T', ' ']) {
                Some((date, time)) => valid_date(date) && valid_time(time),
                None => false,
            },
        }
    }

    /// How well a value of `self` (the column) fits a property of `property`:
    /// the same type is exact, a lossless widening (integer to decimal, date to
    /// date-time) or a rendering as text is half, anything else is none.
    fn compatibility(self, property: Self) -> i64 {
        match (self, property) {
            (a, b) if a == b => ONE,
            (Self::Integer, Self::Decimal) | (Self::Date, Self::DateTime) | (_, Self::Text) => HALF,
            _ => 0,
        }
    }
}

fn valid_date(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return false;
    };
    let number = |part: &str, digits: usize| {
        (part.len() == digits && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    matches!(
        (number(year, 4), number(month, 2), number(day, 2)),
        (Some(_), Some(1..=12), Some(1..=31))
    )
}

fn valid_time(text: &str) -> bool {
    let clock: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    let parts: Vec<&str> = clock.split(':').collect();
    let in_range = |part: &str, max: u32| {
        part.len() == 2 && part.parse::<u32>().is_ok_and(|value| value <= max)
    };
    match parts.as_slice() {
        [h, m] => in_range(h, 23) && in_range(m, 59),
        [h, m, s] => in_range(h, 23) && in_range(m, 59) && in_range(s, 60),
        _ => false,
    }
}

/// A source column: its name, SQL type and a bounded sample of values.
#[derive(Debug, Clone, Copy)]
pub struct SqlColumn<'a> {
    pub name: &'a str,
    pub value_type: ValueType,
    pub samples: &'a [String],
}

/// An ontology property: its IRI and the SHACL datatype it declares, if any.
#[derive(Debug, Clone, Copy)]
pub struct OntologyProperty<'a> {
    pub iri: &'a str,
    pub datatype: Option<ValueType>,
}

/// The three weights in Q32, each in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Weights {
    pub name: i64,
    pub datatype: i64,
    pub sample: i64,
}

impl Weights {
    /// Weights from knob values in micros (`500_000` is `0.5`), clamped to
    /// `[0, 1]`.
    pub fn from_micros(micros: [i64; 3]) -> Self {
        let q32 = |m: i64| ((i128::from(m.clamp(0, 1_000_000)) << 32) / 1_000_000) as i64;
        Self {
            name: q32(micros[0]),
            datatype: q32(micros[1]),
            sample: q32(micros[2]),
        }
    }
}

/// The score of one column against one property, with its parts and the
/// evidence class that backs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappingScore {
    /// The weighted score in Q32, clamped to `[0, ONE]`.
    pub score: i64,
    pub name: i64,
    pub datatype: i64,
    pub sample: i64,
    pub class: PremiseClass,
}

/// The local name of a property IRI: what follows the last `#`, `/` or `:`.
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/', ':']).next().unwrap_or(iri)
}

/// Lowercase alphanumeric tokens, split on punctuation and camelCase humps.
fn tokens(name: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                out.insert(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower && !current.is_empty() {
            out.insert(std::mem::take(&mut current));
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        out.insert(current);
    }
    out
}

/// Token Jaccard of two names in Q32: shared tokens over all tokens.
fn jaccard(left: &str, right: &str) -> i64 {
    let (a, b) = (tokens(left), tokens(right));
    let union = a.union(&b).count() as i128;
    if union == 0 {
        return 0;
    }
    ((a.intersection(&b).count() as i128) << 32) as i64 / union as i64
}

/// The fraction of `samples` that validate against `value_type`, in Q32; no
/// samples validate nothing.
fn sample_fraction(value_type: ValueType, samples: &[String]) -> i64 {
    if samples.is_empty() {
        return 0;
    }
    let valid = samples.iter().filter(|s| value_type.validates(s)).count() as i128;
    ((valid << 32) / samples.len() as i128) as i64
}

fn mul(weight: i64, part: i64) -> i64 {
    ((i128::from(weight) * i128::from(part)) >> 32) as i64
}

/// Whether two names are the same once case and punctuation are removed.
fn same_local_name(column: &str, property: &str) -> bool {
    let flat = |name: &str| -> String {
        name.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let flat_column = flat(column);
    !flat_column.is_empty() && flat_column == flat(local_name(property))
}

/// Score `column` against `property` (MEDULLA-DECISION-ENGINE-R033.2.1).
pub fn score(
    column: &SqlColumn<'_>,
    property: &OntologyProperty<'_>,
    weights: &Weights,
) -> MappingScore {
    let name = jaccard(column.name, local_name(property.iri));
    // A property that declares no datatype has nothing to be compatible with
    // or to validate samples against.
    let datatype = property
        .datatype
        .map_or(0, |declared| column.value_type.compatibility(declared));
    let sample = property
        .datatype
        .map_or(0, |declared| sample_fraction(declared, column.samples));
    let total =
        mul(weights.name, name) + mul(weights.datatype, datatype) + mul(weights.sample, sample);
    let exact_name = same_local_name(column.name, property.iri);
    let class = if exact_name && datatype == ONE {
        PremiseClass::Proof
    } else if property.datatype.is_some() && !column.samples.is_empty() && sample == ONE {
        PremiseClass::Observation
    } else {
        PremiseClass::Claim
    };
    MappingScore {
        score: total.clamp(0, ONE),
        name,
        datatype,
        sample,
        class,
    }
}

/// One property's rank: its index in the input and its score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub score: MappingScore,
}

/// Every property scored and ordered best first; ties break on the property IRI
/// ascending.
pub fn rank(
    column: &SqlColumn<'_>,
    properties: &[OntologyProperty<'_>],
    weights: &Weights,
) -> Vec<Ranked> {
    let mut ranked: Vec<Ranked> = properties
        .iter()
        .enumerate()
        .map(|(index, property)| Ranked {
            index,
            score: score(column, property, weights),
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .score
            .cmp(&a.score.score)
            .then_with(|| properties[a.index].iri.cmp(properties[b.index].iri))
    });
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    fn default_weights() -> Weights {
        Weights::from_micros([500_000, 300_000, 200_000])
    }

    const IRI: &str = "https://knuckles-team.github.io/ontology/core#";

    /// Integer Q32 truncation may lose a few units.
    fn near(actual: i64, expected: i64) {
        assert!(
            (actual - expected).abs() <= 8,
            "{actual} is not within truncation of {expected}"
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R033.2.1
    #[test]
    fn score_and_class_follow_the_rule() {
        let weights = default_weights();
        near(weights.name, ONE / 2);
        near(weights.datatype, ONE * 3 / 10);
        near(weights.sample, ONE / 5);

        // Exact local-name match and matching SHACL datatype: a proof, at the
        // full score (0.5 + 0.3 + 0.2 with every sample valid).
        let ids = samples(&["1", "2", "3"]);
        let column = SqlColumn {
            name: "customer_id",
            value_type: ValueType::from_sql("BIGINT").unwrap(),
            samples: &ids,
        };
        let iri = format!("{IRI}customerId");
        let property = OntologyProperty {
            iri: &iri,
            datatype: ValueType::from_xsd("xsd:integer"),
        };
        let exact = score(&column, &property, &weights);
        assert_eq!(exact.class, PremiseClass::Proof);
        assert_eq!((exact.name, exact.datatype, exact.sample), (ONE, ONE, ONE));
        near(exact.score, ONE);

        // A different name whose samples all validate: an observation. The name
        // Jaccard of {cust, name} against {name} is one half, the datatype is
        // exact: 0.5 x 0.5 + 0.3 + 0.2 = 0.75.
        let names = samples(&["a", "b"]);
        let column = SqlColumn {
            name: "cust_name",
            value_type: ValueType::from_sql("varchar(80)").unwrap(),
            samples: &names,
        };
        let iri = format!("{IRI}name");
        let property = OntologyProperty {
            iri: &iri,
            datatype: ValueType::from_xsd("http://www.w3.org/2001/XMLSchema#string"),
        };
        let observed = score(&column, &property, &weights);
        assert_eq!(observed.class, PremiseClass::Observation);
        near(observed.name, ONE / 2);
        near(observed.score, ONE * 3 / 4);

        // The same name with one sample that does not validate: the match is
        // exact but a failing sample leaves the datatype only compatible by
        // declaration, so an exact name and datatype is still a proof.
        let mixed = samples(&["1", "x"]);
        let column = SqlColumn {
            name: "age",
            value_type: ValueType::Integer,
            samples: &mixed,
        };
        let iri = format!("{IRI}age");
        let property = OntologyProperty {
            iri: &iri,
            datatype: Some(ValueType::Integer),
        };
        let proof_with_a_bad_sample = score(&column, &property, &weights);
        assert_eq!(proof_with_a_bad_sample.class, PremiseClass::Proof);
        assert_eq!(proof_with_a_bad_sample.sample, ONE / 2);
        // 0.5 + 0.3 + 0.2 x 0.5 = 0.9
        near(proof_with_a_bad_sample.score, ONE * 9 / 10);

        // A different name with a failing sample is only a claim.
        let column = SqlColumn {
            name: "years",
            value_type: ValueType::Integer,
            samples: &mixed,
        };
        let claim = score(&column, &property, &weights);
        assert_eq!(claim.class, PremiseClass::Claim);
        assert_eq!(claim.name, 0);
        near(claim.score, ONE * 3 / 10 + ONE / 10);

        // No samples never make an observation, and a property without a
        // declared datatype earns no datatype or sample credit.
        let none: Vec<String> = Vec::new();
        let column = SqlColumn {
            name: "years",
            value_type: ValueType::Integer,
            samples: &none,
        };
        assert_eq!(
            score(&column, &property, &weights).class,
            PremiseClass::Claim
        );
        let untyped = OntologyProperty {
            iri: &iri,
            datatype: None,
        };
        let untyped_score = score(&column, &untyped, &weights);
        assert_eq!(untyped_score.class, PremiseClass::Claim);
        assert_eq!((untyped_score.datatype, untyped_score.sample), (0, 0));

        // The datatype credit: exact, a lossless widening or rendering as text,
        // or none.
        assert_eq!(ValueType::Integer.compatibility(ValueType::Integer), ONE);
        assert_eq!(
            ValueType::Integer.compatibility(ValueType::Decimal),
            ONE / 2
        );
        assert_eq!(ValueType::Date.compatibility(ValueType::DateTime), ONE / 2);
        assert_eq!(ValueType::Boolean.compatibility(ValueType::Text), ONE / 2);
        assert_eq!(ValueType::Text.compatibility(ValueType::Integer), 0);
        assert_eq!(ValueType::Decimal.compatibility(ValueType::Integer), 0);
    }

    // spec: MEDULLA-DECISION-ENGINE-R033.2.1
    #[test]
    fn weights_scale_the_parts_and_ties_break_on_the_iri() {
        let ids = samples(&["1"]);
        let column = SqlColumn {
            name: "id",
            value_type: ValueType::Integer,
            samples: &ids,
        };
        let by_name_only = Weights::from_micros([1_000_000, 0, 0]);
        let iri_b = format!("{IRI}b_id");
        let iri_a = format!("{IRI}a_id");
        let iri_exact = format!("{IRI}id");
        let properties = [
            OntologyProperty {
                iri: &iri_b,
                datatype: Some(ValueType::Integer),
            },
            OntologyProperty {
                iri: &iri_a,
                datatype: Some(ValueType::Integer),
            },
            OntologyProperty {
                iri: &iri_exact,
                datatype: Some(ValueType::Integer),
            },
        ];
        let ranked = rank(&column, &properties, &by_name_only);
        // `id` matches exactly (Jaccard 1); `a_id` and `b_id` tie at one half
        // and order by IRI ascending.
        assert_eq!(
            ranked.iter().map(|r| r.index).collect::<Vec<_>>(),
            vec![2, 1, 0]
        );
        assert_eq!(ranked[1].score.score, ranked[2].score.score);

        // A zero weight removes its part from the score entirely.
        let no_weights = Weights::from_micros([0, 0, 0]);
        assert_eq!(score(&column, &properties[2], &no_weights).score, 0);
        // Weights above one are clamped into the knob's range, and the score
        // stays a unit value.
        let clamped = Weights::from_micros([2_000_000, 1_000_000, 1_000_000]);
        assert_eq!(clamped.name, ONE);
        assert_eq!(score(&column, &properties[2], &clamped).score, ONE);
    }

    // spec: MEDULLA-DECISION-ENGINE-R033.2.1
    #[test]
    fn sql_and_xsd_types_and_sample_forms_are_a_closed_table() {
        for (sql, expected) in [
            ("INT", ValueType::Integer),
            ("bigint(20)", ValueType::Integer),
            ("DOUBLE PRECISION", ValueType::Decimal),
            ("numeric(10, 2)", ValueType::Decimal),
            ("varchar(255)", ValueType::Text),
            ("bool", ValueType::Boolean),
            ("date", ValueType::Date),
            ("timestamp with time zone", ValueType::DateTime),
        ] {
            assert_eq!(ValueType::from_sql(sql), Some(expected), "{sql}");
        }
        assert_eq!(ValueType::from_sql("geometry"), None);
        assert_eq!(
            ValueType::from_xsd("xsd:nonNegativeInteger"),
            Some(ValueType::Integer)
        );
        assert_eq!(
            ValueType::from_xsd("xsd:dateTime"),
            Some(ValueType::DateTime)
        );
        assert_eq!(ValueType::from_xsd("xsd:hexBinary"), None);

        assert!(ValueType::Integer.validates(" 42 "));
        assert!(!ValueType::Integer.validates("4.2"));
        assert!(ValueType::Decimal.validates("4.2"));
        assert!(!ValueType::Decimal.validates("NaN"));
        assert!(ValueType::Boolean.validates("TRUE"));
        assert!(!ValueType::Boolean.validates("yes"));
        assert!(ValueType::Date.validates("2026-11-13"));
        assert!(!ValueType::Date.validates("2026-13-01"));
        assert!(!ValueType::Date.validates("13/11/2026"));
        assert!(ValueType::DateTime.validates("2026-11-13T08:30:00Z"));
        assert!(ValueType::DateTime.validates("2026-11-13 08:30"));
        assert!(!ValueType::DateTime.validates("2026-11-13"));
        assert!(!ValueType::DateTime.validates("2026-11-13T25:00"));
    }
}
