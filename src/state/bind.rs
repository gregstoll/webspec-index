//! Argument binding (§9): aligns a call's argument region with its callee's
//! signature. Derived at consumption time, never stored.
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::parse::steps::{AnchorTarget, InlineToken, InlineTokenKind, LinkSpan, TextSpan};
use crate::state::grammar::{Encoded, Env, Parser};
use crate::state::ir::{call_id, ArgName, Call, Expr, NamedForm, StatementSource};
use crate::state::model::{Passing, Signature, SignatureForm, TemplatePiece};

/// A call with the part of its statement source it binds over: the
/// `state_calls.call_json` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRecord {
    pub call: Call,
    pub source: StatementSource,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub call: String,
    pub callee: AnchorTarget,
    pub args: Vec<ArgBinding>,
    pub initializers: Vec<(String, Expr)>,
    pub confidence: Confidence,
    pub issues: Vec<BindingIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgBinding {
    pub param: u32,
    pub value: ArgValue,
    pub via: BoundVia,
    pub span: Option<TextSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgValue {
    Expr(Expr),
    Body(String),
    Default(Expr),
    Unbound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundVia {
    Literal(String),
    ListPosition,
    Name(ArgName),
    Receiver,
    BodyArgument,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Exact,
    Partial,
    Unbound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingIssue {
    MissingRequiredArgument(String),
    UnknownNamedArgument(String),
    OpaqueArgument(String),
    TrailingText(String),
    ReceiverUnbound,
    TemplateMismatch,
    NoSignature,
    NoTemplate,
    MissingSpec,
}

pub fn issue_code(issue: &BindingIssue) -> &'static str {
    match issue {
        BindingIssue::MissingRequiredArgument(_) => "missing_required_argument",
        BindingIssue::UnknownNamedArgument(_) => "unknown_named_argument",
        BindingIssue::OpaqueArgument(_) => "opaque_argument",
        BindingIssue::TrailingText(_) => "trailing_text",
        BindingIssue::ReceiverUnbound => "receiver_unbound",
        BindingIssue::TemplateMismatch => "template_mismatch",
        BindingIssue::NoSignature => "no_signature",
        BindingIssue::NoTemplate => "no_template",
        BindingIssue::MissingSpec => "missing_spec",
    }
}

/// The stored form of `call`: its source cut to `call.span.start ..
/// call.region.end`, every span rebased to that start. Region links keep
/// their `target` and `generator_id`; `href` and `link_type` are dropped.
pub fn call_record(call: &Call, source: &StatementSource) -> CallRecord {
    let start = call.span.start.min(source.text.len());
    let end = call.region.end.clamp(start, source.text.len());
    let inside = |span: TextSpan| start <= span.start && span.end <= end;
    let rebase = |span: TextSpan| TextSpan {
        start: span.start.saturating_sub(start),
        end: span.end.saturating_sub(start),
    };
    let tokens = source
        .tokens
        .iter()
        .filter(|token| inside(token.span))
        .map(|token| InlineToken {
            span: rebase(token.span),
            ..token.clone()
        })
        .collect();
    let links = source
        .links
        .iter()
        .filter(|link| inside(link.span))
        .map(|link| LinkSpan {
            span: rebase(link.span),
            href: String::new(),
            link_type: None,
            ..link.clone()
        })
        .collect();
    let mut stored = call.clone();
    stored.span = rebase(call.span);
    stored.region = rebase(call.region);
    for arg in &mut stored.named {
        arg.span = rebase(arg.span);
    }
    CallRecord {
        call: stored,
        source: StatementSource {
            id: source.id.clone(),
            subject: source.subject.clone(),
            context: source.context.clone(),
            text: source.text[start..end].to_string(),
            tokens,
            links,
        },
        offset: start,
    }
}

/// Binds `call`'s arguments to `signature`'s parameters (§9.1). Spans are in
/// the coordinates of `source`, the call's statement source or its stored
/// region source.
pub fn bind(call: &Call, source: &StatementSource, signature: &Signature) -> Binding {
    let enc = Encoded::new(source);
    let env = Env::default();
    let mut binder = Binder {
        call,
        source,
        signature,
        masked: masked_text(source),
        parser: Parser::new(&enc, source, &env),
        args: (0..signature.params.len())
            .map(|index| ArgBinding {
                param: index as u32,
                value: ArgValue::Unbound,
                via: BoundVia::Default,
                span: None,
            })
            .collect(),
        issues: Vec::new(),
        initializers: Vec::new(),
    };
    let confidence = binder.run();
    Binding {
        call: call.id.clone(),
        callee: call
            .callee
            .target
            .clone()
            .unwrap_or_else(|| signature.algorithm.clone()),
        args: binder.args,
        initializers: binder.initializers,
        confidence,
        issues: binder.issues,
    }
}

/// Separators a named-argument part starts after, longest first.
const NAMED_LEADS: [&str; 5] = [", with ", " with ", ", and ", " and ", ", "];
/// Top-level list separators, longest first.
const LIST_SEPARATORS: [&str; 3] = [", and ", ", ", " and "];
/// Words that may introduce a nested call at the start of an argument.
const CALL_LEADS: [&[&str]; 3] = [
    &["the result of "],
    &["running ", "performing ", "invoking ", "calling "],
    &["the "],
];
/// The only keyword equivalence class (§16.4).
const GIVEN_CLASS: [&str; 2] = ["given", "with"];

/// How a positional slot is found in the alignment text.
enum Sep {
    /// Directly after the callee: at the cursor.
    First,
    Literal(String),
    List,
}

/// A slot whose separator was found: the argument runs from `arg_start` to
/// the next found slot's `sep_start`.
struct Found {
    param: usize,
    sep_start: usize,
    arg_start: usize,
    via: BoundVia,
    first: bool,
}

struct Binder<'a> {
    call: &'a Call,
    source: &'a StatementSource,
    signature: &'a Signature,
    /// `source.text` with every link, variable and code byte replaced by `x`.
    masked: String,
    parser: Parser<'a>,
    args: Vec<ArgBinding>,
    issues: Vec<BindingIssue>,
    initializers: Vec<(String, Expr)>,
}

impl Binder<'_> {
    fn run(&mut self) -> Confidence {
        let Some(template) = self.signature.template.as_ref() else {
            self.named(false);
            self.issues.push(BindingIssue::NoTemplate);
            return Confidence::Unbound;
        };
        self.named(true);
        self.receiver();
        self.body();
        if self.signature.form == SignatureForm::Ecmarkup {
            self.ecmarkup();
        } else {
            self.positional(&template.pieces);
        }
        self.defaults();
        self.confidence()
    }

    fn is_bound(&self, param: usize) -> bool {
        matches!(
            self.args[param].value,
            ArgValue::Expr(_) | ArgValue::Body(_)
        )
    }

    fn set(&mut self, param: usize, value: ArgValue, via: BoundVia, span: Option<TextSpan>) {
        if let ArgValue::Expr(Expr::Opaque { text }) = &value {
            self.issues.push(BindingIssue::OpaqueArgument(text.clone()));
        }
        self.args[param] = ArgBinding {
            param: param as u32,
            value,
            via,
            span,
        };
    }

    fn missing(&mut self, param: usize) {
        let name = self.signature.params[param].name.clone();
        self.issues
            .push(BindingIssue::MissingRequiredArgument(name));
    }

    /// Rule 2: named arguments by parameter anchor, else by exact name.
    fn named(&mut self, bind: bool) {
        for arg in &self.call.named {
            if let NamedForm::AttributeInit { member } = &arg.form {
                if let Expr::Opaque { text } = &arg.value {
                    self.issues.push(BindingIssue::OpaqueArgument(text.clone()));
                }
                self.initializers.push((member.clone(), arg.value.clone()));
                continue;
            }
            let params = &self.signature.params;
            let flag = arg.form == NamedForm::FlagSet;
            let param = match &arg.name {
                ArgName::ParamLink {
                    target: Some(target),
                    ..
                } => params
                    .iter()
                    .position(|p| p.anchor.as_ref() == Some(target)),
                ArgName::ParamLink { target: None, .. } => None,
                ArgName::Var(name) | ArgName::Text(name) => {
                    params.iter().position(|p| same_name(&p.name, name, flag))
                }
            };
            match param {
                Some(param) if !self.is_bound(param) => {
                    if bind {
                        self.set(
                            param,
                            ArgValue::Expr(arg.value.clone()),
                            BoundVia::Name(arg.name.clone()),
                            Some(arg.span),
                        );
                    }
                }
                _ => {
                    let name = self.arg_name_text(&arg.name);
                    self.issues.push(BindingIssue::UnknownNamedArgument(name));
                }
            }
        }
    }

    fn arg_name_text(&self, name: &ArgName) -> String {
        match name {
            ArgName::Var(name) | ArgName::Text(name) => name.clone(),
            ArgName::ParamLink { link_id, target } => self
                .source
                .links
                .iter()
                .find(|link| &link.id == link_id)
                .map(|link| link.visible_text.clone())
                .or_else(|| target.as_ref().map(|t| t.anchor.clone()))
                .unwrap_or_default(),
        }
    }

    /// Rule 3: the receiver binds to the `Receiver` parameters, or to the
    /// only required non-steps positional parameter of a `To`-like
    /// signature.
    fn receiver(&mut self) {
        let params = &self.signature.params;
        let receivers: Vec<usize> = (0..params.len())
            .filter(|&i| params[i].passing == Passing::Receiver && !self.is_bound(i))
            .collect();
        let Some(receiver) = self.call.receiver.clone() else {
            for param in receivers {
                if !params[param].optional {
                    self.missing(param);
                }
            }
            return;
        };
        let to_like = matches!(
            self.signature.form,
            SignatureForm::To | SignatureForm::WhenStepsSay | SignatureForm::GivenList
        );
        match receiver {
            _ if receivers.len() == 1 => self.set(
                receivers[0],
                ArgValue::Expr(receiver),
                BoundVia::Receiver,
                None,
            ),
            Expr::List(items) if receivers.len() > 1 && items.len() == receivers.len() => {
                for (param, item) in receivers.into_iter().zip(items) {
                    self.set(param, ArgValue::Expr(item), BoundVia::Receiver, None);
                }
            }
            receiver if receivers.is_empty() && to_like => {
                let candidates: Vec<usize> = (0..params.len())
                    .filter(|&i| {
                        matches!(params[i].passing, Passing::Positional { .. })
                            && !params[i].optional
                            && !is_steps_type(&params[i].type_text)
                            && !self.is_bound(i)
                    })
                    .collect();
                match candidates[..] {
                    [param] => self.set(param, ArgValue::Expr(receiver), BoundVia::Receiver, None),
                    _ => self.issues.push(BindingIssue::ReceiverUnbound),
                }
            }
            _ => self.issues.push(BindingIssue::ReceiverUnbound),
        }
    }

    /// Rule 4: the first steps-typed parameter takes the first body argument.
    fn body(&mut self) {
        let Some(body) = self.call.body_args.first() else {
            return;
        };
        let param = (0..self.signature.params.len())
            .find(|&i| !self.is_bound(i) && is_steps_type(&self.signature.params[i].type_text));
        if let Some(param) = param {
            self.set(
                param,
                ArgValue::Body(body.clone()),
                BoundVia::BodyArgument,
                None,
            );
        }
    }

    /// The alignment text: the region up to the named-argument part.
    fn alignment(&self) -> (usize, usize) {
        let len = self.source.text.len();
        let mut start = self.call.region.start.min(len);
        let mut end = self.call.region.end.clamp(start, len);
        if let Some(named) = self.call.named.iter().map(|arg| arg.span.start).min() {
            end = named.clamp(start, end);
            let before = &self.masked[start..end];
            if let Some(lead) = NAMED_LEADS.iter().find(|lead| before.ends_with(*lead)) {
                end -= lead.len();
                if self.masked[start..end].ends_with(", and") {
                    end -= ", and".len();
                }
            }
        }
        start += callee_noun_len(&self.masked[start..end]);
        const CATCHING: &str = ", catching any exceptions";
        let (_, trimmed) = self.trim(start, end);
        if self.masked[start..trimmed].ends_with(CATCHING) {
            end = trimmed - CATCHING.len();
        }
        let body_bound = self
            .args
            .iter()
            .any(|arg| matches!(arg.value, ArgValue::Body(_)));
        if body_bound {
            if let Some(intro) = body_intro().find(&self.masked[start..end]) {
                end = start + intro.start();
            }
        }
        (start, end)
    }

    /// Rule 5: positional alignment in template order.
    fn positional(&mut self, pieces: &[TemplatePiece]) {
        let (start, end) = self.alignment();
        let slots = self.slots(pieces);
        let mut cursor = start;
        let mut found: Vec<Found> = Vec::new();
        let mut unlocated: Vec<usize> = Vec::new();
        for (index, (sep, param)) in slots.into_iter().enumerate() {
            let bound = self.is_bound(param);
            let juxtaposed = match &sep {
                Sep::Literal(literal) if index == 0 && GIVEN_CLASS.contains(&literal.as_str()) => {
                    self.juxtaposed_argument(start, end)
                }
                _ => None,
            };
            let located = match &sep {
                _ if juxtaposed.is_some() => juxtaposed.map(|at| (at, at)),
                Sep::First if index == 0 => Some((cursor, self.after_given(cursor, end))),
                Sep::First => None,
                Sep::Literal(literal) => self.find_literal(literal, cursor, end),
                Sep::List => self.find_list_separator(cursor, end),
            };
            let sep = if juxtaposed.is_some() {
                Sep::First
            } else {
                sep
            };
            let Some((sep_start, mut arg_start)) = located else {
                if bound {
                    continue;
                }
                if self.signature.params[param].optional {
                    break;
                }
                unlocated.push(param);
                continue;
            };
            arg_start += self.keyword_continuation(&sep, arg_start, end);
            cursor = arg_start;
            let first = matches!(sep, Sep::First);
            found.push(Found {
                param,
                sep_start,
                arg_start,
                via: match sep {
                    Sep::First => BoundVia::Literal(String::new()),
                    Sep::Literal(literal) => BoundVia::Literal(literal),
                    Sep::List => BoundVia::ListPosition,
                },
                first,
            });
        }
        for param in unlocated {
            match self.steps_named_after_to(&found, param, end) {
                Some(steps) => found.push(steps),
                None => self.missing(param),
            }
        }
        let leading_end = match found.first() {
            Some(first) if first.first => start,
            Some(first) => first.sep_start,
            None => end,
        };
        self.trailing(start, leading_end);
        for index in 0..found.len() {
            let arg_end = found.get(index + 1).map_or(end, |next| next.sep_start);
            let Found {
                param,
                arg_start,
                ref via,
                ..
            } = found[index];
            let via = via.clone();
            if self.is_bound(param) {
                if !matches!(self.args[param].value, ArgValue::Body(_)) {
                    self.trailing(arg_start, arg_end);
                }
                continue;
            }
            match self.argument(param, arg_start, arg_end) {
                Some((value, span)) => self.set(param, ArgValue::Expr(value), via, Some(span)),
                None if !self.signature.params[param].optional => self.missing(param),
                None => {}
            }
        }
    }

    /// Where the first argument starts when the call omits the template's
    /// leading `given`/`with` and names the argument right after the callee
    /// ("canonicalize a sanitizer element *item*"): the region's first word
    /// is a variable or `this`, not the keyword or another preposition.
    fn juxtaposed_argument(&self, start: usize, end: usize) -> Option<usize> {
        let (at, end) = self.trim(start, end);
        if at >= end {
            return None;
        }
        let variable = self
            .source
            .tokens
            .iter()
            .any(|token| token.kind == InlineTokenKind::Variable && token.span.start == at);
        let text = &self.source.text[at..end];
        let this = text
            .strip_prefix("this")
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
        (variable || this).then_some(at)
    }

    /// The length of a keyword continuing a located separator before the
    /// argument: `by` after `given` ("run the classic script given by *el*'s
    /// result"), `given`/`with` after a list separator ("*a*, given *b*, *c*").
    fn keyword_continuation(&self, sep: &Sep, arg_start: usize, end: usize) -> usize {
        let rest = &self.masked[arg_start..end];
        let keyword = match sep {
            Sep::Literal(literal) if GIVEN_CLASS.contains(&literal.as_str()) => Some(" by"),
            Sep::List => GIVEN_CLASS.iter().copied().find(|k| rest.starts_with(k)),
            _ => None,
        };
        keyword
            .filter(|k| rest.starts_with(k) && rest[k.len()..].starts_with(' '))
            .map_or(0, str::len)
    }

    /// A steps parameter no separator located, passed as a steps variable
    /// after the last argument: "… given *global* to run *afterPopulated*".
    fn steps_named_after_to(&self, found: &[Found], param: usize, end: usize) -> Option<Found> {
        let params = &self.signature.params;
        let position = |p: usize| match params[p].passing {
            Passing::Positional { index } => Some(index),
            _ => None,
        };
        let last = found.last()?;
        if !is_steps_type(&params[param].type_text) || position(param)? < position(last.param)? {
            return None;
        }
        let text = &self.masked[last.arg_start..end];
        let (lead, offset) = [" to run ", " to perform "]
            .iter()
            .find_map(|lead| text.find(lead).map(|offset| (*lead, offset)))?;
        let arg_start = last.arg_start + offset + lead.len();
        let (at, arg_end) = self.trim(arg_start, end);
        let variable = self.source.tokens.iter().any(|token| {
            token.kind == InlineTokenKind::Variable
                && token.span.start == at
                && token.span.end == arg_end
        });
        variable.then(|| Found {
            param,
            sep_start: last.arg_start + offset,
            arg_start,
            via: BoundVia::Literal(lead.trim().to_string()),
            first: false,
        })
    }

    /// Where a first argument the template names right after the callee
    /// starts when the call introduces it with `given`/`with` anyway
    /// ("convert to a list of name-value pairs with *entry list*").
    fn after_given(&self, start: usize, end: usize) -> usize {
        let (at, end) = self.trim(start, end);
        GIVEN_CLASS
            .iter()
            .find_map(|keyword| {
                let rest = self.masked[at..end].strip_prefix(keyword)?;
                let rest = rest
                    .strip_prefix(" by")
                    .filter(|r| r.starts_with(' '))
                    .unwrap_or(rest);
                rest.starts_with(' ').then(|| end - rest.len() + 1)
            })
            .filter(|&arg| arg < end)
            .unwrap_or(start)
    }

    /// The positional slots after the callee with their separators.
    fn slots(&self, pieces: &[TemplatePiece]) -> Vec<(Sep, usize)> {
        let Some(callee) = pieces.iter().position(|p| *p == TemplatePiece::Callee) else {
            return Vec::new();
        };
        let mut slots = Vec::new();
        let mut sep = Sep::First;
        for piece in &pieces[callee + 1..] {
            match piece {
                TemplatePiece::Literal(literal) => {
                    sep = match sep {
                        Sep::Literal(before) => Sep::Literal(format!("{before} {literal}")),
                        _ => Sep::Literal(literal.clone()),
                    }
                }
                TemplatePiece::ListSep => sep = Sep::List,
                TemplatePiece::Slot(index) => {
                    let param = self
                        .signature
                        .params
                        .iter()
                        .position(|p| p.passing == Passing::Positional { index: *index });
                    let current = std::mem::replace(&mut sep, Sep::List);
                    if let Some(param) = param {
                        slots.push((current, param));
                    }
                }
                TemplatePiece::Head(_) | TemplatePiece::Callee | TemplatePiece::NamedGroup(_) => {}
            }
        }
        slots
    }

    /// Rule 7: an ecmarkup call's parenthesized list, split at top-level
    /// `, ` and bound by position.
    fn ecmarkup(&mut self) {
        let (start, end) = self.alignment();
        let text = &self.masked[start..end];
        let (inner_start, inner_end) = match text.find('(') {
            Some(open) => {
                let open = start + open;
                let close = self.closing_paren(open, end);
                (open + 1, close)
            }
            None => (end, end),
        };
        let mut items = Vec::new();
        let mut item_start = inner_start;
        let mut depth = 0usize;
        for (offset, ch) in self.masked[inner_start..inner_end].char_indices() {
            let pos = inner_start + offset;
            match ch {
                '(' | '\u{AB}' => depth += 1,
                ')' | '\u{BB}' => depth = depth.saturating_sub(1),
                ',' if depth == 0 && self.masked[pos..].starts_with(", ") => {
                    items.push((item_start, pos));
                    item_start = pos + 1;
                }
                _ => {}
            }
        }
        if !self.masked[item_start..inner_end].trim().is_empty() || !items.is_empty() {
            items.push((item_start, inner_end));
        }
        let mut positional: Vec<(u32, usize)> = self
            .signature
            .params
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match p.passing {
                Passing::Positional { index } => Some((index, i)),
                _ => None,
            })
            .collect();
        positional.sort();
        let mut items = items.into_iter();
        for (_, param) in positional {
            match items.next().and_then(|(s, e)| self.argument(param, s, e)) {
                Some((value, span)) => self.set(
                    param,
                    ArgValue::Expr(value),
                    BoundVia::ListPosition,
                    Some(span),
                ),
                None if !self.signature.params[param].optional => self.missing(param),
                None => {}
            }
        }
        for (s, e) in items {
            self.trailing(s, e);
        }
    }

    /// The `)` closing the `(` at `open`, or `end`.
    fn closing_paren(&self, open: usize, end: usize) -> usize {
        let mut depth = 0usize;
        for (offset, ch) in self.masked[open..end].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return open + offset;
                    }
                }
                _ => {}
            }
        }
        end
    }

    /// Rule 8: unbound optional parameters take their stated default.
    fn defaults(&mut self) {
        for (index, param) in self.signature.params.iter().enumerate() {
            if param.optional && !self.is_bound(index) {
                self.args[index] = ArgBinding {
                    param: index as u32,
                    value: param
                        .default
                        .clone()
                        .map_or(ArgValue::Unbound, ArgValue::Default),
                    via: BoundVia::Default,
                    span: None,
                };
            }
        }
    }

    /// Rule 9.
    fn confidence(&mut self) -> Confidence {
        if self.issues.is_empty() {
            return Confidence::Exact;
        }
        let bound = (0..self.args.len()).any(|i| self.is_bound(i));
        if bound || self.signature.params.is_empty() {
            return Confidence::Partial;
        }
        let explained = self.issues.iter().any(|issue| {
            matches!(
                issue,
                BindingIssue::MissingRequiredArgument(_)
                    | BindingIssue::ReceiverUnbound
                    | BindingIssue::NoTemplate
                    | BindingIssue::NoSignature
                    | BindingIssue::MissingSpec
            )
        });
        if !explained {
            self.issues.push(BindingIssue::TemplateMismatch);
        }
        Confidence::Unbound
    }

    /// Text of `start..end` left over by the alignment.
    fn trailing(&mut self, start: usize, end: usize) {
        let (start, end) = self.trim(start, end);
        if start < end {
            let text = self.source.text[start..end].to_string();
            self.issues.push(BindingIssue::TrailingText(text));
        }
    }

    /// `start..end` without surrounding whitespace and trailing `,`/`.`.
    fn trim(&self, start: usize, end: usize) -> (usize, usize) {
        let text = &self.source.text[start..end];
        let rest = text.trim_start();
        let start = start + (text.len() - rest.len());
        let rest = rest.trim_end_matches(|c: char| c.is_whitespace() || c == ',' || c == '.');
        (start, start + rest.len())
    }

    /// Rule 6: the argument in `start..end` as an expression, with its
    /// trimmed span; `None` when it is empty.
    fn argument(&mut self, param: usize, start: usize, end: usize) -> Option<(Expr, TextSpan)> {
        let (mut start, mut end) = self.trim(start, end);
        // "*x* if given" forwards *x* when present; "*a*, *b*, and *c*
        // respectively" lists values in parameter order.
        for suffix in [" if given", " respectively"] {
            if self.source.text[start..end].ends_with(suffix) {
                (start, end) = self.trim(start, end - suffix.len());
            }
        }
        if let Some(open) = self.trailing_remark(start, end) {
            (start, end) = self.trim(start, open);
        }
        let text = &self.source.text;
        let param = &self.signature.params[param];
        let led_by = |phrase: &str| {
            !phrase.is_empty()
                && start + phrase.len() + 1 < end
                && self.masked[start..].starts_with(phrase)
                && self.masked[start + phrase.len()..].starts_with(' ')
        };
        if led_by(&param.type_text) {
            start += param.type_text.len() + 1;
        } else if led_by(&param.name) {
            start += param.name.len() + 1;
        }
        if start >= end {
            return None;
        }
        let span = TextSpan { start, end };
        let mut at = start;
        for leads in CALL_LEADS {
            if let Some(lead) = leads.iter().find(|lead| text[at..end].starts_with(*lead)) {
                at += lead.len();
            }
        }
        let nested = self
            .source
            .links
            .iter()
            .find(|link| link.span.start == at && link.span.end <= end)
            .map(|link| call_id(&self.source.id, &link.id))
            .filter(|id| self.call.nested.contains(id));
        if let Some(id) = nested {
            return Some((Expr::Call(id), span));
        }
        let enc = self.parser.enc;
        let expr = self.parser.expr_at(enc.to_enc(start), enc.to_enc(end));
        Some((expr, span))
    }

    /// The `(` of a parenthesized prose remark ending `start..end` after a
    /// value ("*title* (which could be the empty string)"): its first
    /// character is a plain lowercase letter, so tuples like `(*x*, *y*)`
    /// never qualify.
    fn trailing_remark(&self, start: usize, end: usize) -> Option<usize> {
        if !self.masked[start..end].ends_with(')') {
            return None;
        }
        let mut depth = 0usize;
        for (offset, ch) in self.masked[start..end].char_indices().rev() {
            match ch {
                ')' => depth += 1,
                '(' => {
                    depth -= 1;
                    if depth == 0 {
                        let open = start + offset;
                        let text = &self.source.text[open + 1..end];
                        let plain = text.starts_with(|c: char| c.is_ascii_lowercase())
                            && self.masked.as_bytes()[open + 1] == text.as_bytes()[0];
                        let spaced = self.masked[start..open].ends_with(' ');
                        let value = !self.masked[start..open].trim().is_empty();
                        return (plain && spaced && value).then_some(open);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The first whole-word, case-insensitive occurrence of `literal` in
    /// `from..end` of the masked text: its start and end.
    fn find_literal(&self, literal: &str, from: usize, end: usize) -> Option<(usize, usize)> {
        let words: Vec<&str> = literal.split_whitespace().collect();
        if words.is_empty() {
            return None;
        }
        let bytes = self.masked.as_bytes();
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        (from..end).find_map(|pos| {
            if pos > 0 && is_word(bytes[pos - 1]) && is_word(bytes[pos]) {
                return None;
            }
            let mut at = pos;
            for (index, word) in words.iter().enumerate() {
                if index > 0 {
                    at = (bytes.get(at) == Some(&b' ')).then_some(at + 1)?;
                }
                let spellings: &[&str] = if GIVEN_CLASS.contains(word) {
                    &GIVEN_CLASS
                } else {
                    std::slice::from_ref(word)
                };
                let spelling = spellings.iter().find(|s| {
                    bytes
                        .get(at..at + s.len())
                        .is_some_and(|b| b.eq_ignore_ascii_case(s.as_bytes()))
                })?;
                at += spelling.len();
            }
            let closes = at <= end
                && !(at > pos
                    && is_word(bytes[at - 1])
                    && bytes.get(at).is_some_and(|&b| is_word(b)));
            closes.then_some((pos, at))
        })
    }

    /// The next top-level list separator in `from..end`: its start and end.
    fn find_list_separator(&self, from: usize, end: usize) -> Option<(usize, usize)> {
        let mut depth = 0usize;
        for (offset, ch) in self.masked[from..end].char_indices() {
            let pos = from + offset;
            match ch {
                '(' | '\u{AB}' => depth += 1,
                ')' | '\u{BB}' => depth = depth.saturating_sub(1),
                _ if depth == 0 => {
                    if let Some(sep) = LIST_SEPARATORS
                        .iter()
                        .find(|sep| self.masked[pos..end].starts_with(*sep))
                    {
                        return Some((pos, pos + sep.len()));
                    }
                }
                _ => {}
            }
        }
        None
    }
}

/// The phrase ending an argument region that introduces the body passed as
/// a body argument: "… to run the following steps:", "… and these steps:".
fn body_intro() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i),? (?:to (?:run|perform|execute) |and )(?:the following|these|the) (?:sub)?steps?\s*[:.]?\s*$",
        )
        .unwrap()
    })
}

/// The length of a leading `steps`/`algorithm` that completes the callee's
/// name ("run the module type from module request steps given …"): only
/// when an argument keyword or the region end follows it.
fn callee_noun_len(region: &str) -> usize {
    let rest = region.trim_start();
    let lead = region.len() - rest.len();
    ["steps", "algorithm"]
        .iter()
        .find_map(|noun| {
            let after = rest.strip_prefix(noun)?;
            let tail = after.trim_end_matches(|c: char| c.is_whitespace() || c == ',' || c == '.');
            let keyword = GIVEN_CLASS.iter().any(|k| {
                after
                    .strip_prefix(' ')
                    .and_then(|a| a.strip_prefix(k))
                    .is_some_and(|a| a.starts_with(' '))
            });
            (tail.is_empty() || keyword).then_some(lead + noun.len())
        })
        .unwrap_or(0)
}

/// A parameter type phrase naming steps or an algorithm.
fn is_steps_type(type_text: &str) -> bool {
    type_text.contains("steps") || type_text.contains("algorithm")
}

/// A named argument's name against a parameter name; a flag's name matches
/// with or without a trailing ` flag`.
fn same_name(param: &str, name: &str, flag: bool) -> bool {
    if param == name {
        return true;
    }
    let bare = |s: &'_ str| s.strip_suffix(" flag").unwrap_or(s).to_string();
    flag && bare(param) == bare(name)
}

/// `source.text` with every link, variable and code token replaced by `x`
/// bytes, so words inside them never match and offsets stay valid.
fn masked_text(source: &StatementSource) -> String {
    let text = &source.text;
    let mut bytes = text.as_bytes().to_vec();
    let spans = source.links.iter().map(|link| link.span).chain(
        source
            .tokens
            .iter()
            .filter(|token| {
                matches!(
                    token.kind,
                    InlineTokenKind::Variable | InlineTokenKind::Code | InlineTokenKind::Literal
                )
            })
            .map(|token| token.span),
    );
    for span in spans {
        let mut start = span.start.min(text.len());
        let mut end = span.end.clamp(start, text.len());
        while !text.is_char_boundary(start) {
            start -= 1;
        }
        while !text.is_char_boundary(end) {
            end += 1;
        }
        bytes[start..end].fill(b'x');
    }
    String::from_utf8(bytes).expect("whole characters are masked")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::model::Literal;
    use crate::state::testing::*;

    /// Bind the calls of `anchor`'s step `step` against the signatures of the same document,
    /// through the stored form (`call_record`), as the query layer does. Spans are region-relative.
    fn bound(html: &str, spec: &str, anchor: &str, step: &str) -> Vec<(Binding, CallRecord)> {
        let state = extract_html(html, spec);
        calls_at(&state, anchor, step)
            .into_iter()
            .map(|call| {
                let source = state
                    .sources
                    .iter()
                    .find(|s| s.id == call.source_id)
                    .unwrap();
                let record = call_record(call, source);
                let target = call.callee.target.as_ref().unwrap();
                let sig = state
                    .signatures
                    .iter()
                    .find(|s| &s.algorithm == target)
                    .unwrap_or_else(|| panic!("no signature for {target:?}"));
                (bind(&record.call, &record.source, sig), record)
            })
            .collect()
    }
    fn arg(b: &Binding, i: usize) -> (&ArgValue, &BoundVia) {
        (&b.args[i].value, &b.args[i].via)
    }

    #[test]
    fn g2_location_object_navigate_calls_navigate_exactly() {
        let (b, _) = bound(NAV_HTML, "HTML", "location-object-navigate", "4").remove(0);
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(
            arg(&b, 0),
            (
                &ArgValue::Expr(var("navigable")),
                &BoundVia::Literal(String::new())
            )
        );
        assert_eq!(
            arg(&b, 1),
            (&ArgValue::Expr(var("url")), &BoundVia::Literal("to".into()))
        );
        assert_eq!(
            arg(&b, 2),
            (
                &ArgValue::Expr(var("sourceDocument")),
                &BoundVia::Literal("using".into())
            )
        );
        assert_eq!(
            b.args[3].value,
            ArgValue::Expr(Expr::Literal(Literal::Bool(true)))
        );
        assert!(matches!(
            &b.args[3].via,
            BoundVia::Name(ArgName::ParamLink { .. })
        ));
        assert_eq!(b.args[4].value, ArgValue::Expr(var("historyHandling")));
        assert_eq!(
            arg(&b, 5),
            (
                &ArgValue::Default(Expr::Literal(Literal::String(String::new()))),
                &BoundVia::Default
            )
        );
        let (b, _) = bound(NAV_HTML, "HTML", "dom-location-assign", "2").remove(0);
        assert_eq!(b.confidence, Confidence::Exact);
        assert_eq!(b.args[0].value, ArgValue::Expr(Expr::This));
        assert_eq!(b.args[1].value, ArgValue::Expr(var("urlRecord")));
        assert!(
            matches!(&b.args[2].value, ArgValue::Default(Expr::EnumValue { text, .. }) if text == "auto")
        );
    }

    #[test]
    fn anonymous_body_intro_ends_the_argument_before_a_body_argument() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="q">queue a global task</dfn> on a <a href="#task-source">task source</a> <var>source</var>, with a <a href="#global-object">global object</a> <var>global</var> and a series of steps <var>steps</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="c">c</dfn> given a <a href="#global-object">global object</a> <var>global</var>:</p><ol>
<li><p><a href="#q">Queue a global task</a> on <var>source</var> given <var>global</var> to run the following steps:</p><ol><li><p>Return.</p></li></ol></li>
<li><p><a href="#q">Queue a global task</a> on <var>source</var> given <var>global</var> to resolve <var>p</var>.</p></li>
</ol></div>"##;
        let (b, _) = bound(html, "HTML", "c", "1").remove(0);
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(b.args[0].value, ArgValue::Expr(var("source")));
        assert_eq!(b.args[1].value, ArgValue::Expr(var("global")));
        assert!(matches!(b.args[2].value, ArgValue::Body(_)));
        let (b, _) = bound(html, "HTML", "c", "2").remove(0);
        assert_eq!(b.confidence, Confidence::Partial);
        assert_eq!(b.args[2].value, ArgValue::Unbound);
    }

    #[test]
    fn juxtaposed_first_argument_stands_for_an_omitted_given() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="canon">canonicalize a name</dfn> given a string <var>name</var> and a string <var>ns</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="c">c</dfn> given a string <var>item</var>:</p><ol>
<li><p><a href="#canon">Canonicalize a name</a> <var>item</var>.</p></li>
<li><p><a href="#canon">Canonicalize a name</a> <var>item</var> and <var>x</var>.</p></li>
<li><p><a href="#canon">Canonicalize a name</a> this and <var>x</var>.</p></li>
<li><p><a href="#canon">Canonicalize a name</a> from <var>item</var> and <var>x</var>.</p></li>
<li><p><a href="#canon">Canonicalize a name</a> the item and <var>x</var>.</p></li>
</ol></div>"##;
        let first = |step: &str| bound(html, "HTML", "c", step).remove(0).0;
        let b = first("1");
        assert_eq!(b.confidence, Confidence::Partial);
        assert_eq!(
            arg(&b, 0),
            (
                &ArgValue::Expr(var("item")),
                &BoundVia::Literal(String::new())
            )
        );
        assert_eq!(
            b.issues,
            vec![BindingIssue::MissingRequiredArgument("ns".into())]
        );
        let b = first("2");
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(b.args[0].value, ArgValue::Expr(var("item")));
        assert_eq!(
            arg(&b, 1),
            (&ArgValue::Expr(var("x")), &BoundVia::ListPosition)
        );
        let b = first("3");
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(b.args[0].value, ArgValue::Expr(Expr::This));
        for step in ["4", "5"] {
            let b = first(step);
            assert_ne!(b.confidence, Confidence::Exact, "step {step}");
            assert_ne!(b.args[0].value, ArgValue::Expr(var("item")), "step {step}");
        }
    }

    #[test]
    fn keyword_before_a_bare_slot_callee_noun_and_trailing_remarks_are_not_arguments() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="conv">convert to pairs</dfn> <var>list</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>The <dfn id="mtype">module type from request</dfn> steps, given a string <var>request</var>, are as follows:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="text">create a text</dfn> given a document <var>doc</var> and a string <var>data</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="trav">traverse</dfn> given a navigable <var>n</var>, an integer <var>delta</var>, and an optional document <var>sourceDocument</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="c">c</dfn> given a list <var>l</var>:</p><ol>
<li><p><a href="#conv">Convert to pairs</a> with <var>l</var>.</p></li>
<li><p>Set <var>m</var> to the result of running the <a href="#mtype">module type from request</a> steps given <var>l</var>.</p></li>
<li><p><a href="#text">Create a text</a> given <var>d</var> and <var>t</var> (which could be the empty string).</p></li>
<li><p><a href="#text">Create a text</a> given <var>d</var> and (<var>t</var>, <var>u</var>).</p></li>
<li><p><a href="#trav">Traverse</a> given <var>n</var>, <var>l</var>, and with <a href="#sourcedocument">sourceDocument</a> set to <var>d</var>.</p></li>
<li><p>Let <var>m</var> be the result of running the <a href="#mtype">module type from request</a> steps of <var>l</var>.</p></li>
</ol></div>"##;
        let first = |step: &str| {
            bound(html, "HTML", "c", step)
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("no call in step {step}"))
                .0
        };
        let b = first("1");
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(b.args[0].value, ArgValue::Expr(var("l")));
        let b = first("2");
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(b.args[0].value, ArgValue::Expr(var("l")));
        let b = first("3");
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(b.args[1].value, ArgValue::Expr(var("t")));
        let span = b.args[1].span.unwrap();
        assert_eq!(span.end - span.start, "*t*".len());
        assert_ne!(first("4").confidence, Confidence::Exact);
        let b = first("5");
        assert_eq!(b.args[1].value, ArgValue::Expr(var("l")));
        assert!(
            !b.issues
                .iter()
                .any(|i| matches!(i, BindingIssue::OpaqueArgument(_))),
            "{:?}",
            b.issues
        );
        assert_ne!(first("6").confidence, Confidence::Exact);
    }

    #[test]
    fn keyword_continuations_steps_variables_and_trailing_idioms() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="q">queue a global task</dfn> on a <a href="#task-source">task source</a> <var>source</var>, with a <a href="#global-object">global object</a> <var>global</var> and a series of steps <var>steps</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="run">run a script</dfn> given a script <var>script</var> and an optional boolean <var>rethrow</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="report">report an exception</dfn> <var>exception</var> for a global object <var>global</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="deact">deactivate</dfn> given a document <var>doc</var>, a user involvement <var>ui</var>, and an entry <var>entry</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="c">c</dfn> given a global object <var>g</var>:</p><ol>
<li><p><a href="#q">Queue a global task</a> on <var>s</var> given <var>g</var> to run <var>afterPopulated</var>.</p></li>
<li><p><a href="#q">Queue a global task</a> on <var>s</var> given <var>g</var> to run the steps of <var>a</var>.</p></li>
<li><p><a href="#run">Run a script</a> given by <var>el</var>'s <a href="#result">result</a>.</p></li>
<li><p><a href="#report">Report an exception</a> given by <var>status</var> for <var>g</var>.</p></li>
<li><p><a href="#deact">Deactivate</a> <var>d</var>, given <var>u</var> and <var>e</var>.</p></li>
<li><p><a href="#deact">Deactivate</a> given <var>d</var>, <var>u</var>, and <var>e</var> respectively.</p></li>
<li><p><a href="#run">Run a script</a> given <var>s</var> and <var>r</var> if given, catching any exceptions.</p></li>
</ol></div>"##;
        let first = |step: &str| bound(html, "HTML", "c", step).remove(0).0;
        let b = first("1");
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(b.args[1].value, ArgValue::Expr(var("g")));
        assert_eq!(
            arg(&b, 2),
            (
                &ArgValue::Expr(var("afterPopulated")),
                &BoundVia::Literal("to run".into())
            )
        );
        let b = first("2");
        assert_eq!(b.confidence, Confidence::Partial);
        assert_eq!(b.args[2].value, ArgValue::Unbound);
        let b = first("3");
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert!(matches!(&b.args[0].value, ArgValue::Expr(Expr::Path(_))));
        let b = first("4");
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(b.args[0].value, ArgValue::Expr(var("status")));
        assert_eq!(b.args[1].value, ArgValue::Expr(var("g")));
        for step in ["5", "6"] {
            let b = first(step);
            assert_eq!(
                b.confidence,
                Confidence::Exact,
                "step {step}: {:?}",
                b.issues
            );
            assert_eq!(
                b.args.iter().map(|a| a.value.clone()).collect::<Vec<_>>(),
                vec![
                    ArgValue::Expr(var("d")),
                    ArgValue::Expr(var("u")),
                    ArgValue::Expr(var("e"))
                ],
                "step {step}"
            );
        }
        let b = first("7");
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(b.args[1].value, ArgValue::Expr(var("r")));
    }

    #[test]
    fn argument_led_by_its_parameter_name_binds_the_value() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="split">split</dfn> a node <var>node</var> with integer <var>offset</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="c">c</dfn> given a node <var>n</var>:</p><ol>
<li><p><a href="#split">Split</a> <var>n</var> with offset <var>o</var>.</p></li>
<li><p><a href="#split">Split</a> <var>n</var> with offsets <var>o</var>.</p></li>
</ol></div>"##;
        let (b, _) = bound(html, "HTML", "c", "1").remove(0);
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert_eq!(
            arg(&b, 1),
            (&ArgValue::Expr(var("o")), &BoundVia::Literal("with".into()))
        );
        let (b, _) = bound(html, "HTML", "c", "2").remove(0);
        assert_eq!(b.confidence, Confidence::Partial);
    }

    #[test]
    fn argument_led_by_its_parameter_type_phrase_binds_the_value() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="enqueue">enqueue a reaction</dfn>, given a <a href="#custom-element">custom element</a> <var>element</var>, a callback name <var>callbackName</var>, and a <a href="https://infra.spec.whatwg.org/#list">list</a> <var>args</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="c">c</dfn> given a <a href="#custom-element">custom element</a> <var>element</var>:</p><ol>
<li><p><a href="#enqueue">Enqueue a reaction</a> with <var>element</var>, callback name "<code>connectedCallback</code>", and « ».</p></li>
<li><p><a href="#enqueue">Enqueue a reaction</a> with <var>element</var>, callback "<code>connectedCallback</code>", and « ».</p></li>
</ol></div>"##;
        let (b, _) = bound(html, "HTML", "c", "1").remove(0);
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert!(
            matches!(&b.args[1].value, ArgValue::Expr(Expr::EnumValue { text, .. }) if text == "connectedCallback"),
            "{:?}",
            b.args[1]
        );
        let (b, _) = bound(html, "HTML", "c", "2").remove(0);
        assert_eq!(b.confidence, Confidence::Partial);
    }

    #[test]
    fn g3_insert_and_g4_initialize() {
        let (b, _) = bound(INSERT_DOM, "DOM", "concept-node-replace", "2").remove(0);
        assert_eq!(b.confidence, Confidence::Exact);
        assert_eq!(
            b.args.iter().map(|a| a.value.clone()).collect::<Vec<_>>(),
            vec![
                ArgValue::Expr(var("node")),
                ArgValue::Expr(var("parent")),
                ArgValue::Expr(var("referenceChild")),
                ArgValue::Expr(Expr::Literal(Literal::Bool(true)))
            ]
        );
        let (b, _) = bound(EVENT_DOM, "DOM", "dom-event-initevent", "2").remove(0);
        assert_eq!(b.confidence, Confidence::Exact);
        assert_eq!(b.args[0].value, ArgValue::Expr(Expr::This));
        assert_eq!(
            arg(&b, 2),
            (&ArgValue::Expr(var("bubbles")), &BoundVia::ListPosition)
        );
        assert_eq!(b.args[3].value, ArgValue::Expr(var("cancelable")));
    }

    #[test]
    fn lead_forms_ecmarkup_receivers_and_initializers() {
        let (b, _) = bound(PREPARE_EVENT_HTML, "HTML", "time-marches-on", "1").remove(0);
        assert_eq!(
            b.args[0].value,
            ArgValue::Expr(Expr::EnumValue {
                text: "enter".into(),
                target: None
            })
        );
        assert_eq!(b.args[1].value, ArgValue::Expr(var("cue")));
        assert_eq!(b.confidence, Confidence::Partial);
        assert!(
            matches!(&b.issues[..], [BindingIssue::OpaqueArgument(t)] if t.starts_with("the time"))
        );
        for step in ["1", "2"] {
            let (b, _) = bound(
                GIVENLIST_HTML,
                "HTML",
                "fire-a-traverse-navigate-event",
                step,
            )
            .remove(0);
            assert_eq!(
                b.confidence,
                Confidence::Exact,
                "step {step}: {:?}",
                b.issues
            );
            assert_eq!(b.args[1].value, ArgValue::Expr(var("destination")));
        }
        let (b, _) = bound(ECMA_HTML, "ECMA-262", "sec-caller", "1").remove(0);
        assert_eq!(
            b.args.iter().map(|a| a.value.clone()).collect::<Vec<_>>(),
            vec![
                ArgValue::Expr(var("s")),
                ArgValue::Expr(Expr::Literal(Literal::String("x".into()))),
                ArgValue::Expr(Expr::Literal(Literal::Number("0".into())))
            ]
        );
        let accessor = bound(FALLBACK_HTML, "HTML", "use-the-base", "1")
            .remove(0)
            .0;
        assert_eq!(
            (
                accessor.args[0].value.clone(),
                accessor.args[0].via.clone(),
                accessor.confidence
            ),
            (
                ArgValue::Expr(var("doc")),
                BoundVia::Receiver,
                Confidence::Exact
            )
        );
        let pred = bound(FALLBACK_HTML, "HTML", "use-the-base", "2")
            .remove(0)
            .0;
        assert_eq!(pred.args[1].value, ArgValue::Expr(var("u")));
        let (b, _) = bound(FIRE_DOM, "DOM", "signal-change", "1").remove(0);
        assert_eq!(
            b.confidence,
            Confidence::Exact,
            ", and return ends the region: {:?}",
            b.issues
        );
        assert_eq!(b.args[1].value, ArgValue::Expr(var("element")));
        assert_eq!(arg(&b, 2), (&ArgValue::Unbound, &BoundVia::Default));
        assert_eq!(
            b.initializers,
            vec![("bubbles".to_string(), Expr::Literal(Literal::Bool(true)))]
        );
    }

    #[test]
    fn error_classes_tuple_and_condition() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="set-the-boundary">set the boundary</dfn> given a boundary point <var>bp</var> and a node <var>n</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="check">check</dfn> given a response <var>response</var> and an origin <var>responseOrigin</var>:</p><ol><li><p>Return true.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="caller">call them</dfn>:</p><ol><li><p>Let <var>r</var> be the result of <a href="#set-the-boundary">setting the boundary</a> given (<var>parent</var>, <var>node</var>’s <a href="#concept-tree-index">index</a>) and <var>y</var>.</p></li>
<li><p>If the result of <a href="#check">check</a> given <var>response</var> and <var>responseOrigin</var> is true, then return.</p></li></ol></div>"##;
        let (b, record) = bound(html, "HTML", "caller", "1").remove(0);
        let span = b.args[0].span.unwrap();
        assert_eq!(
            &record.source.text[span.start..span.end],
            "(*parent*, *node*’s index)"
        );
        assert_eq!(b.args[1].value, ArgValue::Expr(var("y")));
        assert_eq!(b.confidence, Confidence::Partial);
        assert!(record.call.nested.is_empty());
        let (b, record) = bound(html, "HTML", "caller", "2").remove(0);
        assert_eq!(b.confidence, Confidence::Exact, "{:?}", b.issues);
        assert!(
            !record.source.text.contains("is true"),
            "the condition is not part of the call region"
        );
    }

    #[test]
    fn literals_inside_links_and_code_never_match() {
        let html = format!("{NAV_HTML}<div data-algorithm=\"\"><p>To <dfn id=\"t\">t</dfn> given <var>n</var> and <var>d</var>:</p><ol><li><p><a href=\"#navigate\">Navigate</a> <var>n</var> to <a href=\"#the-url-to-use\">the URL to use</a> using <var>d</var>.</p></li></ol></div>");
        let (b, _) = bound(&html, "HTML", "t", "1").remove(0);
        assert_eq!(b.args[0].value, ArgValue::Expr(var("n")));
        assert!(matches!(&b.args[1].value, ArgValue::Expr(Expr::Path(_))));
        assert_eq!(b.args[2].value, ArgValue::Expr(var("d")));
        assert_eq!(b.confidence, Confidence::Exact);
    }

    #[test]
    fn unbalanced_brackets_and_utf8_do_not_panic() {
        let html = format!("{NAV_HTML}<div data-algorithm=\"\"><p>To <dfn id=\"t\">t</dfn>:</p><ol><li><p><a href=\"#navigate\">Navigate</a> <var>n</var> to (<var>u</var>’s « −1 using <var>d</var>.</p></li></ol></div>");
        let (b, record) = bound(&html, "HTML", "t", "1").remove(0);
        let text = &record.source.text;
        for a in &b.args {
            if let Some(s) = a.span {
                assert!(text.is_char_boundary(s.start) && text.is_char_boundary(s.end));
            }
        }
        assert_eq!(b.confidence, Confidence::Partial);
        assert!(b
            .issues
            .iter()
            .any(|i| matches!(i, BindingIssue::OpaqueArgument(_))));
    }

    #[test]
    fn issue_codes_missing_unknown_no_template_mismatch_receiver_trailing() {
        let html = format!("{NAV_HTML}<div data-algorithm=\"\"><p>To <dfn id=\"zap\">zap</dfn>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=\"\"><p>To <dfn id=\"t\">t</dfn> given <var>n</var>:</p><ol>
<li><p><a href=\"#navigate\">Navigate</a> <var>n</var>.</p></li>
<li><p><a href=\"#navigate\">Navigate</a> <var>n</var> to <var>u</var>, with <var>bogus</var> set to true.</p></li>
<li><p><a href=\"#dom-location-assign\">assign</a> <var>n</var>.</p></li>
<li><p><a href=\"#navigate\">Navigate</a>.</p></li>
<li><p><a href=\"#zap\">Zap</a> everything.</p></li>
<li><p>Set <var>n</var>'s <a href=\"#f\">f</a> to be <a href=\"#navigate\">navigated</a>.</p></li></ol></div>");
        let first = |step: &str| bound(&html, "HTML", "t", step).remove(0).0;
        assert!(first("1")
            .issues
            .contains(&BindingIssue::MissingRequiredArgument("url".into())));
        assert!(first("2")
            .issues
            .contains(&BindingIssue::UnknownNamedArgument("bogus".into())));
        assert_eq!(
            (first("3").confidence, first("3").issues.clone()),
            (Confidence::Unbound, vec![BindingIssue::NoTemplate])
        );
        let b = first("4");
        assert_eq!(b.confidence, Confidence::Unbound);
        assert!(
            b.issues
                .contains(&BindingIssue::MissingRequiredArgument("navigable".into()))
                || b.issues.contains(&BindingIssue::TemplateMismatch)
        );
        assert_eq!(
            first("5").issues,
            vec![BindingIssue::TrailingText("everything".into())]
        );
        assert!(first("6").issues.contains(&BindingIssue::ReceiverUnbound));
        assert_eq!(issue_code(&BindingIssue::MissingSpec), "missing_spec");
    }

    #[test]
    fn issue_codes_are_the_serde_tags() {
        let issues = [
            BindingIssue::MissingRequiredArgument("a".into()),
            BindingIssue::UnknownNamedArgument("a".into()),
            BindingIssue::OpaqueArgument("a".into()),
            BindingIssue::TrailingText("a".into()),
            BindingIssue::ReceiverUnbound,
            BindingIssue::TemplateMismatch,
            BindingIssue::NoSignature,
            BindingIssue::NoTemplate,
            BindingIssue::MissingSpec,
        ];
        for issue in issues {
            let json = serde_json::to_value(&issue).unwrap();
            let tag = match &json {
                serde_json::Value::String(tag) => tag.clone(),
                serde_json::Value::Object(map) => map.keys().next().unwrap().clone(),
                other => panic!("{other}"),
            };
            assert_eq!(issue_code(&issue), tag);
        }
        let (b, record) = bound(NAV_HTML, "HTML", "location-object-navigate", "4").remove(0);
        let json = serde_json::to_string(&(&b, &record)).unwrap();
        assert_eq!(
            serde_json::from_str::<(Binding, CallRecord)>(&json).unwrap(),
            (b, record)
        );
    }

    #[test]
    fn full_source_and_stored_record_bind_alike_and_nested_calls_keep_their_ids() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="x">x</dfn> given a string <var>s</var>:</p><ol><li><p>Return <var>s</var>.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="y">y</dfn> given a string <var>t</var>:</p><ol><li><p>Return <var>t</var>.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="c">c</dfn> given a string <var>b</var>:</p><ol><li><p>Let <var>a</var> be the result of <a href="#x">x</a> given the result of <a href="#y">y</a> given <var>b</var>.</p></li></ol></div>"##;
        let state = extract_html(html, "HTML");
        let outer = calls_at(&state, "c", "1")
            .into_iter()
            .find(|c| c.callee.visible_text == "x")
            .unwrap();
        let source = state
            .sources
            .iter()
            .find(|s| s.id == outer.source_id)
            .unwrap();
        let sig = signature(&state, "x");
        let full = bind(outer, source, sig);
        let record = call_record(outer, source);
        assert!(record
            .source
            .links
            .iter()
            .all(|l| l.href.is_empty() && l.link_type.is_none()));
        let stored = bind(&record.call, &record.source, sig);
        assert_eq!(full.confidence, stored.confidence);
        assert_eq!(
            full.args
                .iter()
                .map(|a| a.value.clone())
                .collect::<Vec<_>>(),
            stored
                .args
                .iter()
                .map(|a| a.value.clone())
                .collect::<Vec<_>>()
        );
        let (f, s) = (full.args[0].span.unwrap(), stored.args[0].span.unwrap());
        assert_eq!(
            (f.start, f.end),
            (s.start + record.offset, s.end + record.offset)
        );
        assert!(
            matches!(&full.args[0].value, ArgValue::Expr(Expr::Call(id)) if outer.nested.contains(id))
        );
    }
}
