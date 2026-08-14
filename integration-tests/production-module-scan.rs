use std::{
    collections::HashSet,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
};

use proc_macro2::{LineColumn, TokenStream, TokenTree};
use snafu::Snafu;
use syn::{
    Attribute, Ident, Item, LitStr, Meta, Token,
    parse::{Parse, ParseStream},
    visit::{self, Visit},
};

const TEST_ONLY_CANARY: &str = "#[cfg(test)] mod tests { use diesel::Connection; use sqlx::Row; }
#[cfg(test)] fn fixture() { axum::Router::new(); pool.get(); r2d2::Pool; }
#[cfg(not(test))] fn production() { allowed::call(); }
";
// The canary covers every syntactic position a forbidden symbol can occupy: a
// `use` path, an expression path, and an attribute token stream. No `use` backs
// the fully qualified derive, so `utoipa` is reachable only through the
// attribute's tokens and the canary fails if those stop being scanned.
const PRODUCTION_CANARY: &str = "use sqlx::Row;
#[derive(utoipa::ToSchema)]
struct Response;
fn production() { diesel::sql_query(); pool.get(); r2d2::Pool; sqlx::query(); }
";

#[derive(Clone, Copy, PartialEq, Eq)]
enum TriBool {
    True,
    False,
    Unknown,
}

impl TriBool {
    const fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

struct CfgPredicate {
    name: String,
    arguments: Vec<Self>,
}

impl Parse for CfgPredicate {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name = input.parse::<Ident>()?.to_string();
        let arguments = if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let mut arguments = Vec::new();
            while !content.is_empty() {
                arguments.push(content.parse()?);
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                }
            }
            arguments
        } else {
            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
                input.parse::<LitStr>()?;
            }
            Vec::new()
        };
        Ok(Self { name, arguments })
    }
}

fn evaluate_cfg(predicate: &CfgPredicate) -> TriBool {
    match predicate.name.as_str() {
        "test" => TriBool::False,
        "not" => predicate
            .arguments
            .first()
            .map_or(TriBool::Unknown, |value| evaluate_cfg(value).not()),
        "all" => {
            let mut unknown = false;
            for result in predicate.arguments.iter().map(evaluate_cfg) {
                match result {
                    TriBool::False => return TriBool::False,
                    TriBool::Unknown => unknown = true,
                    TriBool::True => {}
                }
            }
            if unknown {
                TriBool::Unknown
            } else {
                TriBool::True
            }
        }
        "any" => {
            let mut unknown = false;
            for result in predicate.arguments.iter().map(evaluate_cfg) {
                match result {
                    TriBool::True => return TriBool::True,
                    TriBool::Unknown => unknown = true,
                    TriBool::False => {}
                }
            }
            if unknown {
                TriBool::Unknown
            } else {
                TriBool::False
            }
        }
        _ => TriBool::Unknown,
    }
}

fn disabled_in_production(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        if !attribute.path().is_ident("cfg") {
            return false;
        }
        let Meta::List(list) = &attribute.meta else {
            return false;
        };
        syn::parse2::<CfgPredicate>(list.tokens.clone())
            .is_ok_and(|predicate| evaluate_cfg(&predicate) == TriBool::False)
    })
}

fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(value) => &value.attrs,
        Item::Enum(value) => &value.attrs,
        Item::ExternCrate(value) => &value.attrs,
        Item::Fn(value) => &value.attrs,
        Item::ForeignMod(value) => &value.attrs,
        Item::Impl(value) => &value.attrs,
        Item::Macro(value) => &value.attrs,
        Item::Mod(value) => &value.attrs,
        Item::Static(value) => &value.attrs,
        Item::Struct(value) => &value.attrs,
        Item::Trait(value) => &value.attrs,
        Item::TraitAlias(value) => &value.attrs,
        Item::Type(value) => &value.attrs,
        Item::Union(value) => &value.attrs,
        Item::Use(value) => &value.attrs,
        _ => &[],
    }
}

struct Scanner<'a> {
    forbidden: &'a HashSet<&'static str>,
    file: String,
    violations: Vec<Violation>,
}

impl<'a> Scanner<'a> {
    fn new(file: impl Into<String>, forbidden: &'a HashSet<&'static str>) -> Self {
        Self {
            forbidden,
            file: file.into(),
            violations: Vec::new(),
        }
    }

    fn record(&mut self, identifier: &Ident) {
        let symbol = identifier.to_string();
        if self.forbidden.contains(symbol.as_str()) {
            let LineColumn { line, .. } = identifier.span().start();
            self.violations.push(Violation {
                file: self.file.clone(),
                line,
                symbol,
            });
        }
    }

    fn visit_tokens(&mut self, tokens: &TokenStream) {
        for token in tokens.clone() {
            match token {
                TokenTree::Group(group) => self.visit_tokens(&group.stream()),
                TokenTree::Ident(identifier) => self.record(&identifier),
                TokenTree::Literal(_) | TokenTree::Punct(_) => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        if !disabled_in_production(item_attributes(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_ident(&mut self, identifier: &'ast Ident) {
        self.record(identifier);
    }

    /// Every raw token stream syn reaches routes through here, and its default is
    /// a no-op. Overriding it covers macro invocations and attribute token
    /// streams alike, so a fully qualified `#[derive(...)]` cannot evade a rule.
    fn visit_token_stream(&mut self, tokens: &'ast TokenStream) {
        self.visit_tokens(tokens);
    }
}

struct Violation {
    file: String,
    line: usize,
    symbol: String,
}

impl Violation {
    fn message(&self) -> String {
        format!(
            "{}:{}: forbidden production symbol {}",
            self.file, self.line, self.symbol
        )
    }
}

fn scan_source(
    file: &str,
    source: &str,
    forbidden: &HashSet<&'static str>,
) -> Result<Vec<Violation>, syn::Error> {
    let syntax = syn::parse_file(source)?;
    let mut scanner = Scanner::new(file, forbidden);
    scanner.visit_file(&syntax);
    Ok(scanner.violations)
}

fn rust_files(root: &Path, relative: &Path) -> Result<Vec<PathBuf>, ScanError> {
    let absolute = root.join(relative);
    if !absolute.exists() {
        return Ok(Vec::new());
    }
    if absolute.is_file() {
        return Ok(vec![relative.to_path_buf()]);
    }

    let mut entries = fs::read_dir(&absolute)
        .map_err(|source| ScanError::ReadDirectory {
            path: relative.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ScanError::ReadDirectory {
            path: relative.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut files = Vec::new();
    for entry in entries {
        let entry_relative = relative.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|source| ScanError::ReadDirectory {
                path: entry_relative.clone(),
                source,
            })?;
        if file_type.is_dir() {
            files.extend(rust_files(root, &entry_relative)?);
        } else if file_type.is_file() && entry.path().extension().is_some_and(|value| value == "rs")
        {
            files.push(entry_relative);
        }
    }
    Ok(files)
}

fn scan_rule(
    root: &Path,
    files: &[PathBuf],
    forbidden: &HashSet<&'static str>,
) -> Result<Vec<Violation>, ScanError> {
    let mut violations = Vec::new();
    for relative in files {
        let source =
            fs::read_to_string(root.join(relative)).map_err(|source| ScanError::ReadSource {
                path: relative.clone(),
                source,
            })?;
        violations.extend(
            scan_source(&relative.display().to_string(), &source, forbidden).map_err(|source| {
                ScanError::ParseSource {
                    path: relative.clone(),
                    source,
                }
            })?,
        );
    }
    Ok(violations)
}

fn forbidden(symbols: &'static [&'static str]) -> HashSet<&'static str> {
    symbols.iter().copied().collect()
}

fn run_canaries() -> Result<(), ScanError> {
    let test_symbols = forbidden(&["axum", "diesel", "pool", "r2d2", "sqlx"]);
    let test_violations = scan_source("<test-canary>", TEST_ONLY_CANARY, &test_symbols)
        .map_err(|_| ScanError::CanaryFailed)?;
    if !test_violations.is_empty() {
        return Err(ScanError::CanaryFailed);
    }

    let production_symbols = forbidden(&["diesel", "pool", "r2d2", "sqlx", "utoipa"]);
    let production_violations = scan_source(
        "<production-canary>",
        PRODUCTION_CANARY,
        &production_symbols,
    )
    .map_err(|_| ScanError::CanaryFailed)?;
    for symbol in production_symbols {
        if !production_violations
            .iter()
            .any(|violation| violation.symbol == symbol)
        {
            return Err(ScanError::CanaryFailed);
        }
    }
    Ok(())
}

fn run() -> Result<Vec<Violation>, ScanError> {
    run_canaries()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");

    let mut http_files = rust_files(&root, Path::new("server/src/http.rs"))?;
    http_files.extend(rust_files(&root, Path::new("server/src/http"))?);
    http_files.retain(|file| file.file_name().is_none_or(|name| name != "tests.rs"));

    let mut audit_vault_files = rust_files(&root, Path::new("server/src/audit.rs"))?;
    audit_vault_files.extend(rust_files(&root, Path::new("server/src/audit"))?);
    audit_vault_files.extend(rust_files(&root, Path::new("server/src/vault.rs"))?);
    audit_vault_files.extend(rust_files(&root, Path::new("server/src/vault"))?);

    let mut application_files = rust_files(&root, Path::new("server/src/application.rs"))?;
    application_files.extend(rust_files(&root, Path::new("server/src/application"))?);
    let schema_application_file = Path::new("server/src/application/contest.rs");
    let (schema_application_files, application_files): (Vec<PathBuf>, Vec<PathBuf>) =
        application_files
            .into_iter()
            .partition(|file| file == schema_application_file);

    if http_files.is_empty()
        || audit_vault_files.is_empty()
        || application_files.is_empty()
        || schema_application_files.is_empty()
    {
        return Err(ScanError::RuleSourcesMissing);
    }

    let mut violations = scan_rule(
        &root,
        &http_files,
        &forbidden(&["diesel", "pool", "r2d2", "sqlx"]),
    )?;
    violations.extend(scan_rule(&root, &audit_vault_files, &forbidden(&["axum"]))?);
    violations.extend(scan_rule(
        &root,
        &application_files,
        &forbidden(&[
            "axum",
            "diesel",
            "natsume_error_code",
            "pool",
            "prost",
            "r2d2",
            "sqlx",
            "utoipa",
        ]),
    )?);
    // `utoipa` is absent from this one file's set on purpose. The sub-point under
    // rule 2 of `docs/architecture.md` section 6 lets an application read-only
    // value object carry schema-descriptive derives, which describe shape rather
    // than depend on a framework or a persistence adapter. The relaxation is
    // scoped to the only file that needs it because the sibling files hold
    // secret-bearing types, which must never gain a published schema. `axum`
    // stays forbidden everywhere, so the transport framework itself still cannot
    // reach application code.
    violations.extend(scan_rule(
        &root,
        &schema_application_files,
        &forbidden(&[
            "axum",
            "diesel",
            "natsume_error_code",
            "pool",
            "prost",
            "r2d2",
            "sqlx",
        ]),
    )?);
    Ok(violations)
}

fn main() -> ExitCode {
    match run() {
        Ok(violations) if violations.is_empty() => {
            if writeln!(io::stdout().lock(), "module-dependency-scan: ok").is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Ok(violations) => {
            let mut stderr = io::stderr().lock();
            for violation in violations {
                if writeln!(stderr, "{}", violation.message()).is_err() {
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            let _write_result = writeln!(io::stderr().lock(), "module-dependency-scan: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Snafu)]
enum ScanError {
    #[snafu(display("cannot read Rust source {}", path.display()))]
    ReadSource { path: PathBuf, source: io::Error },
    #[snafu(display("cannot parse Rust source {}", path.display()))]
    ParseSource { path: PathBuf, source: syn::Error },
    #[snafu(display("cannot read Rust source directory {}", path.display()))]
    ReadDirectory { path: PathBuf, source: io::Error },
    #[snafu(display("runtime canary failed"))]
    CanaryFailed,
    #[snafu(display("one or more architecture rule source sets are empty"))]
    RuleSourcesMissing,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn violations(source: &str, symbols: &'static [&'static str]) -> Vec<Violation> {
        match scan_source("fixture.rs", source, &forbidden(symbols)) {
            Ok(violations) => violations,
            Err(error) => panic!("fixture did not parse: {error}"),
        }
    }

    #[test]
    fn runtime_canaries_are_valid() {
        assert!(run_canaries().is_ok());
    }

    #[test]
    fn cfg_test_items_are_skipped_but_production_items_are_scanned() {
        assert!(
            violations(
                "#[cfg(test)] fn fixture() { diesel::sql_query(); }",
                &["diesel"]
            )
            .is_empty()
        );
        assert_eq!(
            violations("fn fixture() { diesel::sql_query(); }", &["diesel"]).len(),
            1
        );
    }

    #[test]
    fn cfg_combinators_use_conservative_three_value_logic() {
        assert!(
            violations(
                "#[cfg(all(test, feature = \"x\"))] fn fixture() { diesel::sql_query(); }",
                &["diesel"]
            )
            .is_empty()
        );
        assert_eq!(
            violations(
                "#[cfg(any(test, feature = \"x\"))] fn fixture() { diesel::sql_query(); }",
                &["diesel"]
            )
            .len(),
            1
        );
        assert_eq!(
            violations(
                "#[cfg(not(test))] fn fixture() { diesel::sql_query(); }",
                &["diesel"]
            )
            .len(),
            1
        );
    }

    #[test]
    fn unknown_cfg_predicates_are_scanned() {
        assert_eq!(
            violations(
                "#[cfg(target_os = \"windows\")] fn fixture() { diesel::sql_query(); }",
                &["diesel"]
            )
            .len(),
            1
        );
    }

    #[test]
    fn macro_token_trees_are_scanned_recursively() {
        assert_eq!(
            violations(
                "fn fixture() { wrapper!({ sqlx::query!(\"SELECT 1\") }); }",
                &["sqlx"]
            )
            .len(),
            1
        );
    }

    #[test]
    fn attribute_token_streams_are_scanned() {
        assert_eq!(
            violations(
                "#[derive(diesel::QueryableByName)] struct Row;",
                &["diesel"]
            )
            .len(),
            1
        );
    }

    #[test]
    fn parse_failures_are_not_clean_results() {
        assert!(scan_source("fixture.rs", "fn {", &forbidden(&["diesel"])).is_err());
    }
}
