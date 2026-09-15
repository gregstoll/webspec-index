use crate::effects::model::{
    canonical_json_sha256, EffectValue, Execution, EFFECTS_SCHEMA_VERSION,
};
use regex::Regex;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
#[cfg(feature = "native")]
use std::fs;
use std::path::{Path, PathBuf};
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::Marker;
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError {
    pub file: Option<String>,
    pub location: Option<String>,
    pub message: String,
}

impl CatalogError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            file: None,
            location: None,
            message: message.into(),
        }
    }

    fn file(file: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            file: Some(file.into()),
            location: None,
            message: message.into(),
        }
    }

    fn at(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}")?;
            if let Some(location) = &self.location {
                write!(f, " ({location})")?;
            }
            write!(f, ": ")?;
        }
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CatalogError {}

#[derive(Debug, Clone)]
pub struct CatalogPattern {
    source: String,
    regex: Regex,
}

impl CatalogPattern {
    pub fn as_str(&self) -> &str {
        &self.source
    }

    pub fn regex(&self) -> &Regex {
        &self.regex
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Anchor {
    pub spec: String,
    pub anchor: String,
}

impl Anchor {
    pub fn as_identity(&self) -> String {
        format!("{}#{}", self.spec, self.anchor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    String,
    Boolean,
    Integer,
    Anchor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterDefinition {
    #[serde(rename = "type")]
    pub parameter_type: ParameterType,
    #[serde(rename = "enum", default, skip_serializing_if = "Option::is_none")]
    pub allowed_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectDefinition {
    pub category: String,
    pub label: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterDefinition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    Literal,
    Text,
    Anchor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitValue {
    Unknown,
    Constant(EffectValue),
    Capture {
        capture: String,
        from: Option<CaptureSource>,
    },
}

impl Serialize for EmitValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Unknown => serializer.serialize_none(),
            Self::Constant(value) => value.serialize(serializer),
            Self::Capture { capture, from } => {
                let mut value = serde_json::Map::new();
                value.insert("capture".into(), Value::String(capture.clone()));
                if let Some(from) = from {
                    value.insert(
                        "from".into(),
                        serde_json::to_value(from).map_err(serde::ser::Error::custom)?,
                    );
                }
                value.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for EmitValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::Null => Ok(Self::Unknown),
            Value::String(value) => Ok(Self::Constant(EffectValue::String(value))),
            Value::Bool(value) => Ok(Self::Constant(EffectValue::Boolean(value))),
            Value::Number(value) => value
                .as_i64()
                .map(|value| Self::Constant(EffectValue::Integer(value)))
                .ok_or_else(|| {
                    de::Error::custom("effect constants must be signed 64-bit integers")
                }),
            Value::Object(mut value) => {
                let capture = value
                    .remove("capture")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .ok_or_else(|| {
                        de::Error::custom("capture expression requires string field capture")
                    })?;
                let from = value
                    .remove("from")
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(de::Error::custom)?;
                if let Some(field) = value.keys().next() {
                    return Err(de::Error::custom(format!("unknown capture field {field}")));
                }
                Ok(Self::Capture { capture, from })
            }
            _ => Err(de::Error::custom(
                "effect values must be scalar, null, or a capture expression",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Emit {
    pub kind: String,
    #[serde(default)]
    pub params: BTreeMap<String, EmitValue>,
}

#[derive(Debug, Clone)]
pub struct MatchSpec {
    pub anchor: Option<Anchor>,
    pub subject: Option<Anchor>,
    pub text: Option<CatalogPattern>,
    pub exclude_text: Vec<CatalogPattern>,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub public_id: String,
    pub match_spec: MatchSpec,
    pub emit: Emit,
    pub continuations: Option<Execution>,
    pub description: Option<String>,
    pub expect: Vec<String>,
    pub source_file: String,
}

#[derive(Debug, Clone)]
pub struct ReviewedSummary {
    pub id: String,
    pub public_id: String,
    pub subject: Anchor,
    pub expect_text: CatalogPattern,
    pub reason: String,
    pub emit: Emit,
    pub continuations: Option<Execution>,
    pub source_file: String,
}

#[derive(Debug, Clone)]
pub struct HostImplementation {
    pub id: String,
    pub public_id: String,
    pub environment: String,
    pub operation: Anchor,
    pub implementation: Anchor,
    pub expect_text: CatalogPattern,
    pub reason: String,
    pub source_file: String,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub schema: u32,
    pub id: String,
    pub effects: BTreeMap<String, EffectDefinition>,
    pub rules: Vec<Rule>,
    pub summaries: Vec<ReviewedSummary>,
    pub implementations: Vec<HostImplementation>,
    pub content_digest: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub schema: u32,
    pub packages: Vec<Package>,
    pub effects: BTreeMap<String, EffectDefinition>,
    pub content_digest: String,
}

impl Catalog {
    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.packages
            .iter()
            .flat_map(|package| package.rules.iter())
    }

    pub fn summaries(&self) -> impl Iterator<Item = &ReviewedSummary> {
        self.packages
            .iter()
            .flat_map(|package| package.summaries.iter())
    }

    pub fn implementations(&self) -> impl Iterator<Item = &HostImplementation> {
        self.packages
            .iter()
            .flat_map(|package| package.implementations.iter())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    schema: u32,
    package: String,
    #[serde(default)]
    effects: BTreeMap<String, EffectDefinition>,
    #[serde(default)]
    rules: Vec<RawRule>,
    #[serde(default)]
    summaries: Vec<RawSummary>,
    #[serde(default)]
    implementations: Vec<RawImplementation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    #[serde(rename = "match")]
    match_spec: RawMatchSpec,
    emit: Emit,
    continuations: Option<Execution>,
    description: Option<String>,
    #[serde(default)]
    expect: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMatchSpec {
    anchor: Option<String>,
    subject: Option<String>,
    text: Option<String>,
    #[serde(default)]
    exclude_text: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSummary {
    id: String,
    subject: String,
    expect_text: String,
    reason: String,
    emit: Emit,
    continuations: Option<Execution>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImplementation {
    id: String,
    environment: String,
    operation: String,
    implementation: String,
    expect_text: String,
    reason: String,
}

#[cfg(feature = "native")]
pub fn load_package(path: impl AsRef<Path>) -> Result<Package, CatalogError> {
    let root = path.as_ref();
    let mut paths = Vec::new();
    collect_yaml_files(root, root, &mut paths)?;
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    if paths.is_empty() {
        return Err(CatalogError::new(format!(
            "catalog package {} contains no .yaml or .yml files",
            root.display()
        )));
    }
    let mut files = Vec::with_capacity(paths.len());
    for (relative, absolute) in paths {
        let content = fs::read_to_string(&absolute).map_err(|error| {
            CatalogError::file(relative.clone(), format!("cannot read UTF-8 YAML: {error}"))
        })?;
        files.push((relative, content));
    }
    load_owned_files(files)
}

/// Load one embedded package from `(relative_path, UTF-8 YAML)` pairs.
pub fn load_package_files(files: &[(&str, &str)]) -> Result<Package, CatalogError> {
    load_owned_files(
        files
            .iter()
            .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
            .collect(),
    )
}

/// Combine already loaded packages and perform catalog-wide validation.
pub fn load_catalog(packages: impl IntoIterator<Item = Package>) -> Result<Catalog, CatalogError> {
    let mut packages: Vec<_> = packages.into_iter().collect();
    packages.sort_by(|left, right| left.id.cmp(&right.id));
    let mut package_ids = BTreeSet::new();
    let mut effects = BTreeMap::new();
    for package in &packages {
        if !package_ids.insert(package.id.clone()) {
            return Err(CatalogError::new(format!(
                "duplicate package identifier {}",
                package.id
            )));
        }
        for (kind, definition) in &package.effects {
            if effects.insert(kind.clone(), definition.clone()).is_some() {
                return Err(CatalogError::new(format!(
                    "duplicate global effect kind {kind}"
                )));
            }
        }
    }
    for package in &packages {
        for rule in &package.rules {
            validate_emit(
                &rule.emit,
                &effects,
                rule.match_spec.text.as_ref(),
                &rule.source_file,
                &rule.public_id,
            )?;
        }
        for summary in &package.summaries {
            validate_emit(
                &summary.emit,
                &effects,
                None,
                &summary.source_file,
                &summary.public_id,
            )?;
        }
    }
    let digest_value = Value::Array(
        packages
            .iter()
            .map(|package| json!({"package": package.id, "content_sha256": package.content_digest}))
            .collect(),
    );
    let content_digest = canonical_json_sha256(&digest_value)
        .map_err(|error| CatalogError::new(error.to_string()))?;
    Ok(Catalog {
        schema: EFFECTS_SCHEMA_VERSION,
        packages,
        effects,
        content_digest,
    })
}

/// Load embedded package sets followed by zero or more extension directories.
pub fn load_catalog_sources(
    embedded: &[&[(&str, &str)]],
    additional_dirs: &[PathBuf],
) -> Result<Catalog, CatalogError> {
    let mut packages = Vec::with_capacity(embedded.len() + additional_dirs.len());
    for files in embedded {
        packages.push(load_package_files(files)?);
    }
    #[cfg(feature = "native")]
    for directory in additional_dirs {
        packages.push(load_package(directory)?);
    }
    #[cfg(not(feature = "native"))]
    if let Some(directory) = additional_dirs.first() {
        return Err(CatalogError::new(format!(
            "catalog directory {} requires the native feature",
            directory.display()
        )));
    }
    load_catalog(packages)
}

#[cfg(feature = "native")]
fn collect_yaml_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> Result<(), CatalogError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        CatalogError::new(format!(
            "cannot read catalog directory {}: {error}",
            directory.display()
        ))
    })?;
    for entry in entries {
        let entry = entry
            .map_err(|error| CatalogError::new(format!("cannot read catalog entry: {error}")))?;
        let file_type = entry
            .file_type()
            .map_err(|error| CatalogError::new(error.to_string()))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_yaml_files(root, &path, output)?;
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension == "yaml" || extension == "yml")
        {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| CatalogError::new(error.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            output.push((relative, path));
        }
    }
    Ok(())
}

fn load_owned_files(mut files: Vec<(String, String)>) -> Result<Package, CatalogError> {
    files.sort_by(|left, right| left.0.cmp(&right.0));
    if files.is_empty() {
        return Err(CatalogError::new("catalog package contains no YAML files"));
    }
    let mut raws = Vec::with_capacity(files.len());
    let mut digest_files = Vec::with_capacity(files.len());
    for (path, content) in &files {
        if !(path.ends_with(".yaml") || path.ends_with(".yml")) {
            return Err(CatalogError::file(
                path,
                "embedded catalog file must end in .yaml or .yml",
            ));
        }
        let raw = parse_file(path, content)?;
        digest_files
            .push(json!({"path": path, "content_sha256": sha256_bytes(content.as_bytes())}));
        raws.push((path.clone(), raw));
    }
    let package_id = raws[0].1.package.clone();
    validate_id(&package_id, "package identifier", &raws[0].0)?;
    let mut effects = BTreeMap::new();
    let mut rules = Vec::new();
    let mut summaries = Vec::new();
    let mut implementations = Vec::new();
    let mut ids = BTreeSet::new();
    for (file, raw) in raws {
        if raw.schema != EFFECTS_SCHEMA_VERSION {
            return Err(CatalogError::file(
                &file,
                format!(
                    "unsupported catalog schema {}; expected {}",
                    raw.schema, EFFECTS_SCHEMA_VERSION
                ),
            ));
        }
        if raw.package != package_id {
            return Err(CatalogError::file(
                &file,
                format!("package is {}, expected {package_id}", raw.package),
            ));
        }
        for (kind, definition) in raw.effects {
            validate_effect_definition(&kind, &definition, &file)?;
            if effects.insert(kind.clone(), definition).is_some() {
                return Err(CatalogError::file(
                    &file,
                    format!("duplicate effect definition {kind}"),
                ));
            }
        }
        for raw_rule in raw.rules {
            validate_declaration_id(&raw_rule.id, &mut ids, &file)?;
            let public_id = format!("{package_id}/{}", raw_rule.id);
            let location = format!("rule {public_id}");
            let anchor = raw_rule
                .match_spec
                .anchor
                .as_deref()
                .map(|value| parse_anchor(value, &file, &location))
                .transpose()?;
            let subject = raw_rule
                .match_spec
                .subject
                .as_deref()
                .map(|value| parse_anchor(value, &file, &location))
                .transpose()?;
            let text = raw_rule
                .match_spec
                .text
                .as_deref()
                .map(|value| compile_pattern(value, false, &file, &location))
                .transpose()?;
            if anchor.is_none() && text.is_none() {
                return Err(
                    CatalogError::file(&file, "match requires at least anchor or text")
                        .at(location),
                );
            }
            let exclude_text = raw_rule
                .match_spec
                .exclude_text
                .iter()
                .map(|value| compile_pattern(value, true, &file, &location))
                .collect::<Result<_, _>>()?;
            rules.push(Rule {
                id: raw_rule.id,
                public_id,
                match_spec: MatchSpec {
                    anchor,
                    subject,
                    text,
                    exclude_text,
                },
                emit: raw_rule.emit,
                continuations: raw_rule.continuations,
                description: raw_rule.description,
                expect: raw_rule.expect,
                source_file: file.clone(),
            });
        }
        for raw_summary in raw.summaries {
            validate_declaration_id(&raw_summary.id, &mut ids, &file)?;
            let public_id = format!("{package_id}/{}", raw_summary.id);
            let location = format!("summary {public_id}");
            summaries.push(ReviewedSummary {
                id: raw_summary.id,
                public_id,
                subject: parse_anchor(&raw_summary.subject, &file, &location)?,
                expect_text: compile_pattern(&raw_summary.expect_text, true, &file, &location)?,
                reason: require_text(raw_summary.reason, "reason", &file, &location)?,
                emit: raw_summary.emit,
                continuations: raw_summary.continuations,
                source_file: file.clone(),
            });
        }
        for raw_impl in raw.implementations {
            validate_declaration_id(&raw_impl.id, &mut ids, &file)?;
            let public_id = format!("{package_id}/{}", raw_impl.id);
            let location = format!("implementation {public_id}");
            implementations.push(HostImplementation {
                id: raw_impl.id,
                public_id,
                environment: require_text(raw_impl.environment, "environment", &file, &location)?,
                operation: parse_anchor(&raw_impl.operation, &file, &location)?,
                implementation: parse_anchor(&raw_impl.implementation, &file, &location)?,
                expect_text: compile_pattern(&raw_impl.expect_text, true, &file, &location)?,
                reason: require_text(raw_impl.reason, "reason", &file, &location)?,
                source_file: file.clone(),
            });
        }
    }
    let content_digest = canonical_json_sha256(&Value::Array(digest_files))
        .map_err(|error| CatalogError::new(error.to_string()))?;
    Ok(Package {
        schema: EFFECTS_SCHEMA_VERSION,
        id: package_id,
        effects,
        rules,
        summaries,
        implementations,
        content_digest,
        files: files.into_iter().map(|(path, _)| path).collect(),
    })
}

fn parse_file(path: &str, content: &str) -> Result<RawFile, CatalogError> {
    let mut preflight = YamlPreflight::default();
    Parser::new_from_str(content)
        .load(&mut preflight, true)
        .map_err(|error| CatalogError::file(path, format!("invalid YAML: {error}")))?;
    if let Some(error) = preflight.error {
        return Err(CatalogError::file(path, error));
    }
    if preflight.documents != 1 {
        return Err(CatalogError::file(
            path,
            format!(
                "expected exactly one YAML document, found {}",
                preflight.documents
            ),
        ));
    }
    let mut docs = YamlLoader::load_from_str(content)
        .map_err(|error| CatalogError::file(path, format!("invalid YAML: {error}")))?;
    if docs.len() != 1 {
        return Err(CatalogError::file(
            path,
            "expected exactly one YAML document",
        ));
    }
    let value = yaml_to_json(docs.remove(0), path, "$")?;
    let encoded = serde_json::to_string(&value)
        .map_err(|error| CatalogError::file(path, format!("cannot decode catalog: {error}")))?;
    let mut deserializer = serde_json::Deserializer::from_str(&encoded);
    serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        CatalogError::file(path, format!("invalid catalog fields: {}", error.inner()))
            .at(error.path().to_string())
    })
}

#[derive(Default)]
struct YamlPreflight {
    documents: usize,
    error: Option<String>,
}

impl MarkedEventReceiver for YamlPreflight {
    fn on_event(&mut self, event: Event, marker: Marker) {
        if self.error.is_some() {
            return;
        }
        let at = || format!("line {}, column {}", marker.line() + 1, marker.col() + 1);
        match event {
            Event::DocumentStart => self.documents += 1,
            Event::Alias(_) => {
                self.error = Some(format!("YAML aliases are not supported ({})", at()))
            }
            Event::Scalar(_, _, anchor, tag) => self.check_node(anchor, tag, &at()),
            Event::SequenceStart(anchor, tag) | Event::MappingStart(anchor, tag) => {
                self.check_node(anchor, tag, &at())
            }
            _ => {}
        }
    }
}

impl YamlPreflight {
    fn check_node(&mut self, anchor: usize, tag: Option<yaml_rust2::parser::Tag>, at: &str) {
        if anchor != 0 {
            self.error = Some(format!("YAML anchors are not supported ({at})"));
        } else if let Some(tag) = tag {
            let standard = tag.handle == "tag:yaml.org,2002:"
                && matches!(tag.suffix.as_str(), "str" | "bool" | "int" | "null");
            if !standard {
                self.error = Some(format!("custom YAML tags are not supported ({at})"));
            }
        }
    }
}

fn yaml_to_json(yaml: Yaml, file: &str, location: &str) -> Result<Value, CatalogError> {
    match yaml {
        Yaml::Null => Ok(Value::Null),
        Yaml::Boolean(value) => Ok(Value::Bool(value)),
        Yaml::Integer(value) => Ok(Value::Number(value.into())),
        Yaml::String(value) => Ok(Value::String(value)),
        Yaml::Array(values) => values
            .into_iter()
            .enumerate()
            .map(|(index, value)| yaml_to_json(value, file, &format!("{location}[{index}]")))
            .collect(),
        Yaml::Hash(values) => {
            let mut output = serde_json::Map::new();
            for (key, value) in values {
                let Yaml::String(key) = key else {
                    return Err(
                        CatalogError::file(file, "mapping keys must be strings").at(location)
                    );
                };
                if key == "<<" {
                    return Err(
                        CatalogError::file(file, "YAML merge keys are not supported").at(location),
                    );
                }
                output.insert(
                    key.clone(),
                    yaml_to_json(value, file, &format!("{location}.{key}"))?,
                );
            }
            Ok(Value::Object(output))
        }
        Yaml::Real(_) => {
            Err(CatalogError::file(file, "floating-point values are not supported").at(location))
        }
        Yaml::Alias(_) => {
            Err(CatalogError::file(file, "YAML aliases are not supported").at(location))
        }
        Yaml::BadValue => Err(CatalogError::file(file, "invalid YAML value").at(location)),
    }
}

fn validate_id(value: &str, what: &str, file: &str) -> Result<(), CatalogError> {
    let regex = Regex::new(r"^[a-z0-9][a-z0-9._-]*$").expect("static ID regex");
    if regex.is_match(value) {
        Ok(())
    } else {
        Err(CatalogError::file(
            file,
            format!("invalid {what} {value:?}; expected [a-z0-9][a-z0-9._-]*"),
        ))
    }
}

fn validate_declaration_id(
    id: &str,
    ids: &mut BTreeSet<String>,
    file: &str,
) -> Result<(), CatalogError> {
    validate_id(id, "declaration ID", file)?;
    if ids.insert(id.to_owned()) {
        Ok(())
    } else {
        Err(CatalogError::file(
            file,
            format!("duplicate package-local declaration ID {id}"),
        ))
    }
}

fn validate_effect_definition(
    kind: &str,
    definition: &EffectDefinition,
    file: &str,
) -> Result<(), CatalogError> {
    validate_id(kind, "effect kind", file)?;
    if kind.split('.').count() < 2 || kind.split('.').any(str::is_empty) {
        return Err(CatalogError::file(
            file,
            format!("effect kind {kind:?} must contain at least one dot with nonempty components"),
        ));
    }
    if definition.category.is_empty() || definition.label.is_empty() {
        return Err(CatalogError::file(
            file,
            format!("effect {kind} requires non-empty category and label"),
        ));
    }
    let name_regex = Regex::new(r"^[a-z][a-z0-9_]*$").expect("static parameter regex");
    for (name, parameter) in &definition.parameters {
        if !name_regex.is_match(name) {
            return Err(CatalogError::file(
                file,
                format!("invalid parameter name {name:?} on effect {kind}"),
            ));
        }
        match (&parameter.parameter_type, &parameter.allowed_values) {
            (ParameterType::String, Some(values)) if values.is_empty() => {
                return Err(CatalogError::file(
                    file,
                    format!("string enum {kind}.{name} must not be empty"),
                ));
            }
            (ParameterType::String, Some(values)) => {
                let mut seen = BTreeSet::new();
                if values.iter().any(|value| !seen.insert(value)) {
                    return Err(CatalogError::file(
                        file,
                        format!("string enum {kind}.{name} contains duplicate values"),
                    ));
                }
            }
            (_, Some(_)) => {
                return Err(CatalogError::file(
                    file,
                    format!("enum is only valid for string parameter {kind}.{name}"),
                ))
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_emit(
    emit: &Emit,
    effects: &BTreeMap<String, EffectDefinition>,
    text: Option<&CatalogPattern>,
    file: &str,
    location: &str,
) -> Result<(), CatalogError> {
    let definition = effects.get(&emit.kind).ok_or_else(|| {
        CatalogError::file(file, format!("unknown emitted effect kind {}", emit.kind)).at(location)
    })?;
    for (name, value) in &emit.params {
        let parameter = definition.parameters.get(name).ok_or_else(|| {
            CatalogError::file(
                file,
                format!("unknown parameter {name} for effect {}", emit.kind),
            )
            .at(location)
        })?;
        match value {
            EmitValue::Unknown => {}
            EmitValue::Constant(value) => {
                validate_constant(value, parameter, file, location, name)?
            }
            EmitValue::Capture { capture, from } => {
                let pattern = text.ok_or_else(|| {
                    CatalogError::file(file, format!("capture {capture:?} requires a text pattern"))
                        .at(location)
                })?;
                if pattern
                    .regex()
                    .capture_names()
                    .flatten()
                    .all(|name| name != capture)
                {
                    return Err(CatalogError::file(
                        file,
                        format!("unknown regex capture group {capture:?}"),
                    )
                    .at(location));
                }
                if matches!(
                    parameter.parameter_type,
                    ParameterType::Boolean | ParameterType::Integer
                ) {
                    return Err(CatalogError::file(
                        file,
                        format!(
                            "captures cannot populate {} parameter {name}",
                            match parameter.parameter_type {
                                ParameterType::Boolean => "boolean",
                                _ => "integer",
                            }
                        ),
                    )
                    .at(location));
                }
                let source = from.unwrap_or(match parameter.parameter_type {
                    ParameterType::Anchor => CaptureSource::Anchor,
                    _ => CaptureSource::Literal,
                });
                if parameter.parameter_type == ParameterType::Anchor
                    && source != CaptureSource::Anchor
                {
                    return Err(CatalogError::file(
                        file,
                        format!("anchor parameter {name} requires capture source anchor"),
                    )
                    .at(location));
                }
                if parameter.parameter_type == ParameterType::String
                    && source == CaptureSource::Anchor
                {
                    return Err(CatalogError::file(
                        file,
                        format!("string parameter {name} cannot use capture source anchor"),
                    )
                    .at(location));
                }
            }
        }
    }
    Ok(())
}

fn validate_constant(
    value: &EffectValue,
    parameter: &ParameterDefinition,
    file: &str,
    location: &str,
    name: &str,
) -> Result<(), CatalogError> {
    let valid_type = matches!(
        (value, parameter.parameter_type),
        (
            EffectValue::String(_),
            ParameterType::String | ParameterType::Anchor
        ) | (EffectValue::Boolean(_), ParameterType::Boolean)
            | (EffectValue::Integer(_), ParameterType::Integer)
    );
    if !valid_type {
        return Err(CatalogError::file(
            file,
            format!("constant for parameter {name} has the wrong type"),
        )
        .at(location));
    }
    if let (EffectValue::String(value), Some(allowed)) = (value, &parameter.allowed_values) {
        if !allowed.contains(value) {
            return Err(CatalogError::file(
                file,
                format!("constant {value:?} is outside enum for parameter {name}"),
            )
            .at(location));
        }
    }
    if parameter.parameter_type == ParameterType::Anchor {
        parse_anchor(
            match value {
                EffectValue::String(value) => value,
                _ => unreachable!(),
            },
            file,
            location,
        )?;
    }
    Ok(())
}

fn parse_anchor(value: &str, file: &str, location: &str) -> Result<Anchor, CatalogError> {
    if value.chars().any(char::is_whitespace) || value.matches('#').count() != 1 {
        return Err(CatalogError::file(
            file,
            format!("malformed canonical anchor {value:?}; expected SPEC#anchor"),
        )
        .at(location));
    }
    let (spec, anchor) = value.split_once('#').expect("one delimiter checked");
    if spec.is_empty() || anchor.is_empty() {
        return Err(CatalogError::file(
            file,
            format!("malformed canonical anchor {value:?}; expected SPEC#anchor"),
        )
        .at(location));
    }
    let registry = crate::spec_registry::SpecRegistry::new();
    let canonical_spec = registry
        .infer_base_url_from_spec_name(spec)
        .and_then(|(base, _)| {
            registry.resolve_url(&format!("{}#{anchor}", base.trim_end_matches('#')))
        })
        .map(|(name, _)| name)
        .unwrap_or_else(|| spec.to_owned());
    Ok(Anchor {
        spec: canonical_spec,
        anchor: anchor.to_owned(),
    })
}

fn compile_pattern(
    value: &str,
    allow_empty: bool,
    file: &str,
    location: &str,
) -> Result<CatalogPattern, CatalogError> {
    let regex = Regex::new(value).map_err(|error| {
        CatalogError::file(file, format!("invalid Rust regex {value:?}: {error}")).at(location)
    })?;
    if !allow_empty && regex.is_match("") {
        return Err(CatalogError::file(
            file,
            format!("match regex {value:?} is capable of an empty match"),
        )
        .at(location));
    }
    Ok(CatalogPattern {
        source: value.to_owned(),
        regex,
    })
}

fn require_text(
    value: String,
    field: &str,
    file: &str,
    location: &str,
) -> Result<String, CatalogError> {
    if value.trim().is_empty() {
        Err(CatalogError::file(file, format!("{field} must be non-empty")).at(location))
    } else {
        Ok(value)
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
schema: 1
package: example
effects:
  event.fire:
    category: events
    label: fire an event
    parameters:
      name: {type: string}
rules:
  - id: fire-event
    match: {anchor: DOM#concept-event-fire}
    emit:
      kind: event.fire
      params: {name: null}
"#;

    #[test]
    fn embedded_package_loads() {
        let package = load_package_files(&[("effects/events.yaml", MINIMAL)]).unwrap();
        let catalog = load_catalog([package]).unwrap();
        assert!(catalog.effects.contains_key("event.fire"));
        assert_eq!(
            catalog.rules().next().unwrap().public_id,
            "example/fire-event"
        );
    }

    #[test]
    fn embedded_catalog_loads_with_no_extension_dirs() {
        let catalog = crate::effects::default_catalog(&[]).unwrap();
        assert!(!catalog.packages.is_empty());
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn extension_dirs_are_rejected_without_native() {
        let error = load_catalog_sources(&[], &[std::path::PathBuf::from("/nonexistent")])
            .err()
            .expect("directories need the native feature");
        assert!(error.to_string().contains("native"), "{error}");
    }

    #[test]
    fn yaml_12_boolean_semantics_do_not_treat_yes_as_boolean() {
        let yaml = MINIMAL.replace("name: null", "name: yes");
        let catalog =
            load_catalog([load_package_files(&[("events.yaml", &yaml)]).unwrap()]).unwrap();
        assert_eq!(
            catalog.rules().next().unwrap().emit.params["name"],
            EmitValue::Constant(EffectValue::String("yes".into()))
        );
    }

    #[test]
    fn aliases_and_duplicate_keys_are_rejected() {
        let alias = MINIMAL.replace(
            "category: events",
            "category: &category events\n    extra: *category",
        );
        assert!(load_package_files(&[("alias.yaml", &alias)])
            .unwrap_err()
            .to_string()
            .contains("anchors"));
        let duplicate = MINIMAL.replace("package: example", "package: example\npackage: example");
        assert!(load_package_files(&[("duplicate.yaml", &duplicate)])
            .unwrap_err()
            .to_string()
            .contains("duplicated key"));
    }

    #[test]
    fn catalog_validation_reports_file_and_rule() {
        let yaml = MINIMAL.replace("name: null", "missing: null");
        let error = load_catalog([load_package_files(&[("rules/events.yaml", &yaml)]).unwrap()])
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("rules/events.yaml"));
        assert!(message.contains("example/fire-event"));
        assert!(message.contains("unknown parameter missing"));
    }
}
