use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
};

use proc_macro2::{LineColumn, TokenStream, TokenTree};
use snafu::Snafu;
use syn::{
    Attribute, Ident, Item, LitStr, Meta, Token, UseTree, Visibility,
    parse::{Parse, ParseStream},
    spanned::Spanned as _,
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
const APPLICATION_WIRE_CANARY: &str = "use natsume_device_protocol::generated::{
    CommandState, CommandStatus, ControlEnvelope, control_envelope,
};
use prost::Message;
fn wire() { let _: Option<command::Body> = None; }
";
const APPLICATION_DOMAIN_PROTOCOL_CANARY: &str =
    "use natsume_device_protocol::is_canonical_command_id;
fn domain(value: &str) { let _canonical = is_canonical_command_id(value); }
";
const APPLICATION_WIRE_FORBIDDEN: &[&str] = &[
    "CommandState",
    "CommandStatus",
    "ControlEnvelope",
    "command::Body",
    "control_envelope",
    "generated",
    "prost",
];
const PUBLIC_VISIBILITY_CANARY: &str = "pub struct PublicItem;
pub mod public_module {}
pub(crate) struct CrateItem;
pub(super) struct ParentItem;
pub(in crate) struct ScopedItem;
struct PrivateItem;
";

#[derive(Clone, Copy)]
struct PublicVisibilityAllowance {
    file: &'static str,
    item_kind: &'static str,
    item_name: &'static str,
}

// Deliberately empty: any future exception must name its file, item kind, and
// item in this reviewed list rather than weakening the rule for a whole tree.
const PUBLIC_VISIBILITY_ALLOWLIST: &[PublicVisibilityAllowance] = &[];

const DB_DIRECT_TABLE_CANARY: &str = "use crate::db::schema::{commands, devices};
fn direct_tables() { let _commands = commands::table; let _devices = devices::table; }
";
const DB_ALIAS_TABLE_CANARY: &str =
    "use crate::db::schema::{commands as queued, devices as owners};
fn aliased_mutations() {
    diesel::update(queued::table);
    diesel::delete(owners::table);
}
";
const DB_SQL_JOIN_CANARY: &str = "fn sql_join() {
    diesel::sql_query(\"SELECT * FROM commands c JOIN devices d ON d.device_pk = c.device_pk\");
}
";
const DB_CFG_TEST_CANARY: &str = "use crate::db::schema::{commands, devices};
#[cfg(test)] fn test_only(connection: &mut Connection) {
    diesel::update(commands::table);
    diesel::delete(devices::table);
    connection.transaction(|_| Ok(()));
}
";
const DB_READ_MODEL_WRITE_CANARY: &str = "fn query() {
    diesel::sql_query(\"UPDATE commands SET state = 'created'\");
}
";
const DB_TRANSACTION_CANARY: &str =
    "fn owns_transaction(connection: &mut Connection) { connection.transaction(|_| Ok(())); }";

const CONTEST_ADAPTER_ERROR_CANARY: &str = "fn production() {
    let _: Option<ImportError> = None;
    let _: Option<ContestError> = None;
    let _: Option<ContestPersistenceError> = None;
}
#[cfg(test)] fn fixture() {
    let _: Option<ImportError> = None;
    let _: Option<ContestError> = None;
}
";
const CONTEST_ADAPTER_IMPORT_PATH_CANARY: &str = "use crate::{
    application::import::{
        CurrentAccountProjection, CurrentSeatProjection, ImportError, NewAccountFacts,
    },
};
fn production() {
    let _: Option<crate::application::import::CurrentSeatProjection> = None;
}
#[cfg(test)] mod tests {
    use crate::application::import::{
        CurrentAccountProjection, CurrentSeatProjection, ImportError, NewAccountFacts,
    };
}
";
const AUDIT_ADAPTER_ERROR_CANARY: &str = "fn production() {
    let _: Option<CommandError> = None;
    let _: Option<OperatorError> = None;
    let _: Option<DeviceError> = None;
    let _: Option<ProvisioningError> = None;
    let _: Option<ImportError> = None;
    let _: Option<EnrollmentError> = None;
    let _: Option<AuditPersistenceError> = None;
}
#[cfg(test)] fn fixture() { let _: Option<CommandError> = None; }
";
const ENROLLMENT_REQUEST_ADAPTER_ERROR_CANARY: &str = "fn production() {
    let _: Option<ProvisioningError> = None;
    let _: Option<EnrollmentError> = None;
    let _: Option<EnrollmentRequestPersistenceError> = None;
}
#[cfg(test)] fn fixture() { let _: Option<EnrollmentError> = None; }
";
const PROVISIONING_ADAPTER_ERROR_CANARY: &str = "fn production() {
    let _: Option<ProvisioningError> = None;
    let _: Option<ProvisioningPersistenceError> = None;
}
#[cfg(test)] fn fixture() { let _: Option<ProvisioningError> = None; }
";

const CONTEST_ADAPTER_CALLER_ERRORS: &[&str] = &["ImportError", "ContestError"];
const CONTEST_ADAPTER_IMPORT_PATH: &[&str] = &["crate", "application", "import"];
const AUDIT_ADAPTER_CALLER_ERRORS: &[&str] = &[
    "CommandError",
    "OperatorError",
    "DeviceError",
    "ProvisioningError",
    "ImportError",
    "EnrollmentError",
];
const ENROLLMENT_REQUEST_ADAPTER_CALLER_ERRORS: &[&str] = &["ProvisioningError", "EnrollmentError"];
const PROVISIONING_ADAPTER_CALLER_ERRORS: &[&str] = &["ProvisioningError"];

#[derive(Clone, Copy)]
struct DatabaseBoundaryAllowance {
    file: &'static str,
    function: &'static str,
    rule: ViolationRule,
    tables: &'static [&'static str],
}

// The transition is complete; this constant is asserted empty and transition exceptions are not
// permitted.
const DATABASE_BOUNDARY_ALLOWLIST: &[DatabaseBoundaryAllowance] = &[];

// Multi-table reads are permitted only in modules explicitly reviewed as query/read-model
// adapters. The classifier is module-path based; it never grants function-level exceptions.
const READ_MODEL_MODULE_ALLOWLIST: &[&str] = &[
    "server/src/db/import/query.rs",
    "server/src/db/operator/query.rs",
];

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
                tables: Vec::new(),
                rule: ViolationRule::ForbiddenProductionSymbol,
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

    fn visit_path(&mut self, path: &'ast syn::Path) {
        if self.forbidden.contains("command::Body")
            && path
                .segments
                .iter()
                .zip(path.segments.iter().skip(1))
                .any(|(module, item)| module.ident == "command" && item.ident == "Body")
        {
            let LineColumn { line, .. } = path.span().start();
            self.violations.push(Violation {
                file: self.file.clone(),
                line,
                symbol: "command::Body".to_owned(),
                tables: Vec::new(),
                rule: ViolationRule::ForbiddenProductionSymbol,
            });
        }
        visit::visit_path(self, path);
    }

    /// Every raw token stream syn reaches routes through here, and its default is
    /// a no-op. Overriding it covers macro invocations and attribute token
    /// streams alike, so a fully qualified `#[derive(...)]` cannot evade a rule.
    fn visit_token_stream(&mut self, tokens: &'ast TokenStream) {
        self.visit_tokens(tokens);
    }
}

struct ProductionPathScanner<'a> {
    forbidden_path: &'a [&'static str],
    file: String,
    violations: Vec<Violation>,
}

impl<'a> ProductionPathScanner<'a> {
    fn new(file: impl Into<String>, forbidden_path: &'a [&'static str]) -> Self {
        Self {
            forbidden_path,
            file: file.into(),
            violations: Vec::new(),
        }
    }

    fn record(&mut self, identifier: &Ident) {
        let LineColumn { line, .. } = identifier.span().start();
        self.violations.push(Violation {
            file: self.file.clone(),
            line,
            symbol: self.forbidden_path.join("::"),
            tables: Vec::new(),
            rule: ViolationRule::ForbiddenProductionPath,
        });
    }

    fn is_forbidden(&self, segments: &[String]) -> bool {
        segments.len() >= self.forbidden_path.len()
            && segments
                .iter()
                .zip(self.forbidden_path)
                .all(|(segment, forbidden)| segment == forbidden)
    }

    fn scan_use_tree(&mut self, tree: &UseTree, prefix: &mut Vec<String>) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                if self.is_forbidden(prefix) {
                    self.record(&path.ident);
                } else {
                    self.scan_use_tree(&path.tree, prefix);
                }
                prefix.pop();
            }
            UseTree::Name(name) => {
                prefix.push(name.ident.to_string());
                if self.is_forbidden(prefix) {
                    self.record(&name.ident);
                }
                prefix.pop();
            }
            UseTree::Rename(rename) => {
                prefix.push(rename.ident.to_string());
                if self.is_forbidden(prefix) {
                    self.record(&rename.ident);
                }
                prefix.pop();
            }
            UseTree::Group(group) => {
                for tree in &group.items {
                    self.scan_use_tree(tree, prefix);
                }
            }
            UseTree::Glob(_) => {}
        }
    }
}

impl<'ast> Visit<'ast> for ProductionPathScanner<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        if !disabled_in_production(item_attributes(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.scan_use_tree(&item.tree, &mut Vec::new());
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        if self.is_forbidden(&segments)
            && let Some(segment) = path.segments.iter().nth(self.forbidden_path.len() - 1)
        {
            self.record(&segment.ident);
        }
        visit::visit_path(self, path);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViolationRule {
    ForbiddenProductionSymbol,
    ForbiddenProductionPath,
    BarePublicVisibility,
    MultipleDatabaseTables,
    ReadModelWrite,
    TransactionOpening,
}

struct Violation {
    file: String,
    line: usize,
    symbol: String,
    tables: Vec<String>,
    rule: ViolationRule,
}

impl Violation {
    fn message(&self) -> String {
        match self.rule {
            ViolationRule::ForbiddenProductionSymbol => format!(
                "{}:{}: forbidden production symbol {}",
                self.file, self.line, self.symbol
            ),
            ViolationRule::ForbiddenProductionPath => format!(
                "{}:{}: forbidden production path {}",
                self.file, self.line, self.symbol
            ),
            ViolationRule::BarePublicVisibility => format!(
                "{}:{}: forbidden bare pub visibility on {}",
                self.file, self.line, self.symbol
            ),
            ViolationRule::MultipleDatabaseTables => format!(
                "{}:{}: db function {} references multiple tables: {}",
                self.file,
                self.line,
                self.symbol,
                self.tables.join(", ")
            ),
            ViolationRule::ReadModelWrite => format!(
                "{}:{}: read-model function {} contains a database write{}",
                self.file,
                self.line,
                self.symbol,
                if self.tables.is_empty() {
                    String::new()
                } else {
                    format!(" involving: {}", self.tables.join(", "))
                }
            ),
            ViolationRule::TransactionOpening => format!(
                "{}:{}: db function {} opens a transaction outside db.rs infrastructure",
                self.file, self.line, self.symbol
            ),
        }
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

fn scan_production_path_source(
    file: &str,
    source: &str,
    forbidden_path: &[&'static str],
) -> Result<Vec<Violation>, syn::Error> {
    let syntax = syn::parse_file(source)?;
    let mut scanner = ProductionPathScanner::new(file, forbidden_path);
    scanner.visit_file(&syntax);
    Ok(scanner.violations)
}

fn use_tree_name(tree: &UseTree) -> String {
    match tree {
        UseTree::Path(path) => format!("{}::{}", path.ident, use_tree_name(&path.tree)),
        UseTree::Name(name) => name.ident.to_string(),
        UseTree::Rename(rename) => format!("{} as {}", rename.ident, rename.rename),
        UseTree::Glob(_) => "*".to_owned(),
        UseTree::Group(group) => {
            let names = group
                .items
                .iter()
                .map(use_tree_name)
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{names}}}")
        }
    }
}

fn item_visibility(item: &Item) -> Option<(&Visibility, &'static str, String)> {
    match item {
        Item::Const(value) => Some((&value.vis, "const", value.ident.to_string())),
        Item::Enum(value) => Some((&value.vis, "enum", value.ident.to_string())),
        Item::ExternCrate(value) => Some((&value.vis, "extern crate", value.ident.to_string())),
        Item::Fn(value) => Some((&value.vis, "fn", value.sig.ident.to_string())),
        Item::Mod(value) => Some((&value.vis, "mod", value.ident.to_string())),
        Item::Static(value) => Some((&value.vis, "static", value.ident.to_string())),
        Item::Struct(value) => Some((&value.vis, "struct", value.ident.to_string())),
        Item::Trait(value) => Some((&value.vis, "trait", value.ident.to_string())),
        Item::TraitAlias(value) => Some((&value.vis, "trait alias", value.ident.to_string())),
        Item::Type(value) => Some((&value.vis, "type", value.ident.to_string())),
        Item::Union(value) => Some((&value.vis, "union", value.ident.to_string())),
        Item::Use(value) => Some((&value.vis, "use", use_tree_name(&value.tree))),
        Item::ForeignMod(_) | Item::Impl(_) | Item::Macro(_) | Item::Verbatim(_) | _ => None,
    }
}

fn public_visibility_is_allowed(
    file: &str,
    item_kind: &str,
    item_name: &str,
    allowlist: &[PublicVisibilityAllowance],
) -> bool {
    allowlist.iter().any(|allowance| {
        allowance.file == file
            && allowance.item_kind == item_kind
            && allowance.item_name == item_name
    })
}

fn scan_public_visibility_source(
    file: &str,
    source: &str,
    allowlist: &[PublicVisibilityAllowance],
) -> Result<Vec<Violation>, syn::Error> {
    let syntax = syn::parse_file(source)?;
    let mut violations = Vec::new();
    scan_public_visibility_items(file, &syntax.items, allowlist, &mut violations);
    Ok(violations)
}

fn scan_public_visibility_items(
    file: &str,
    items: &[Item],
    allowlist: &[PublicVisibilityAllowance],
    violations: &mut Vec<Violation>,
) {
    for item in items {
        // Inline modules cap effective visibility, but a bare `pub` inside them is
        // still one re-export away from escaping, so the rule recurses.
        if let Item::Mod(module) = item
            && let Some((_, nested)) = &module.content
        {
            scan_public_visibility_items(file, nested, allowlist, violations);
        }
        // `#[macro_export]` hoists to the crate root regardless of module
        // visibility, so it is a bare-public surface in its own right.
        if let Item::Macro(value) = item
            && value
                .attrs
                .iter()
                .any(|attribute| attribute.path().is_ident("macro_export"))
        {
            let name = value
                .ident
                .as_ref()
                .map_or_else(|| "macro".to_owned(), ToString::to_string);
            if !public_visibility_is_allowed(file, "macro", &name, allowlist) {
                let LineColumn { line, .. } = value.mac.path.span().start();
                violations.push(Violation {
                    file: file.to_owned(),
                    line,
                    symbol: format!("macro {name}"),
                    tables: Vec::new(),
                    rule: ViolationRule::BarePublicVisibility,
                });
            }
            continue;
        }
        let Some((Visibility::Public(public), item_kind, item_name)) = item_visibility(item) else {
            continue;
        };
        if public_visibility_is_allowed(file, item_kind, &item_name, allowlist) {
            continue;
        }
        let LineColumn { line, .. } = public.span.start();
        violations.push(Violation {
            file: file.to_owned(),
            line,
            symbol: format!("{item_kind} {item_name}"),
            tables: Vec::new(),
            rule: ViolationRule::BarePublicVisibility,
        });
    }
}

fn schema_table_names(root: &Path) -> Result<HashSet<String>, ScanError> {
    let relative = PathBuf::from("server/src/db/schema.rs");
    let source =
        fs::read_to_string(root.join(&relative)).map_err(|source| ScanError::ReadSource {
            path: relative.clone(),
            source,
        })?;
    let syntax = syn::parse_file(&source).map_err(|source| ScanError::ParseSource {
        path: relative,
        source,
    })?;
    let tables = syntax
        .items
        .into_iter()
        .filter_map(|item| {
            let Item::Macro(item) = item else {
                return None;
            };
            if item
                .mac
                .path
                .segments
                .last()
                .is_none_or(|segment| segment.ident != "table")
            {
                return None;
            }
            item.mac.tokens.into_iter().find_map(|token| match token {
                TokenTree::Ident(identifier) => Some(identifier.to_string()),
                TokenTree::Group(_) | TokenTree::Literal(_) | TokenTree::Punct(_) => None,
            })
        })
        .collect::<HashSet<_>>();
    if tables.is_empty() {
        return Err(ScanError::RuleSourcesMissing);
    }
    Ok(tables)
}

fn schema_aliases(syntax: &syn::File, schema_tables: &HashSet<String>) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for item in &syntax.items {
        let Item::Use(item_use) = item else {
            continue;
        };
        if disabled_in_production(&item_use.attrs) {
            continue;
        }
        collect_schema_aliases(&item_use.tree, &mut Vec::new(), schema_tables, &mut aliases);
    }
    aliases
}

fn collect_schema_aliases(
    tree: &UseTree,
    prefix: &mut Vec<String>,
    schema_tables: &HashSet<String>,
    aliases: &mut HashMap<String, String>,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_schema_aliases(&path.tree, prefix, schema_tables, aliases);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let original = name.ident.to_string();
            prefix.push(original.clone());
            record_schema_alias(prefix, &original, schema_tables, aliases);
            prefix.pop();
        }
        UseTree::Rename(rename) => {
            prefix.push(rename.ident.to_string());
            record_schema_alias(prefix, &rename.rename.to_string(), schema_tables, aliases);
            prefix.pop();
        }
        UseTree::Glob(_) => {
            if prefix.last().is_some_and(|segment| segment == "schema") {
                for table in schema_tables {
                    aliases.insert(table.clone(), table.clone());
                }
            }
        }
        UseTree::Group(group) => {
            for child in &group.items {
                collect_schema_aliases(child, prefix, schema_tables, aliases);
            }
        }
    }
}

fn record_schema_alias(
    path: &[String],
    local_name: &str,
    schema_tables: &HashSet<String>,
    aliases: &mut HashMap<String, String>,
) {
    let Some(schema_index) = path.iter().position(|segment| segment == "schema") else {
        return;
    };
    let Some(table) = path.get(schema_index.saturating_add(1)) else {
        return;
    };
    if schema_tables.contains(table) {
        aliases.insert(local_name.to_owned(), table.clone());
    }
}

struct DatabaseFunctionScanner<'a> {
    aliases: &'a HashMap<String, String>,
    schema_tables: &'a HashSet<String>,
    tables: BTreeSet<String>,
    first_write_line: Option<usize>,
    first_transaction_line: Option<usize>,
}

impl<'a> DatabaseFunctionScanner<'a> {
    fn new(aliases: &'a HashMap<String, String>, schema_tables: &'a HashSet<String>) -> Self {
        Self {
            aliases,
            schema_tables,
            tables: BTreeSet::new(),
            first_write_line: None,
            first_transaction_line: None,
        }
    }

    fn record_path(&mut self, path: &syn::Path) {
        let segments = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        if let Some(first) = segments.first()
            && let Some(table) = self.aliases.get(first)
        {
            self.tables.insert(table.clone());
        }
        if let Some(schema_index) = segments.iter().position(|segment| segment == "schema")
            && let Some(table) = segments.get(schema_index.saturating_add(1))
            && self.schema_tables.contains(table)
        {
            self.tables.insert(table.clone());
        }
        let Some(last) = segments.last() else {
            return;
        };
        if matches!(
            last.as_str(),
            "delete" | "insert_into" | "replace_into" | "update"
        ) {
            let LineColumn { line, .. } = path.span().start();
            self.first_write_line.get_or_insert(line);
        }
    }

    fn record_sql(&mut self, literal: &LitStr) {
        let words = sql_words(&literal.value());
        let LineColumn { line, .. } = literal.span().start();
        for (index, word) in words.iter().enumerate() {
            let table_start = match word.as_str() {
                "from" | "join" | "update" => Some(index.saturating_add(1)),
                "into"
                    if index > 0 && matches!(words[index - 1].as_str(), "insert" | "replace") =>
                {
                    Some(index.saturating_add(1))
                }
                _ => None,
            };
            if let Some(table_start) = table_start
                && let Some(table) = sql_table_after(&words, table_start, self.schema_tables)
            {
                self.tables.insert(table);
            }
            if matches!(
                word.as_str(),
                "alter" | "create" | "drop" | "insert" | "replace" | "update"
            ) || (word == "delete"
                && words
                    .get(index.saturating_add(1))
                    .is_some_and(|v| v == "from"))
            {
                self.first_write_line.get_or_insert(line);
            }
            if matches!(word.as_str(), "begin" | "savepoint") {
                self.first_transaction_line.get_or_insert(line);
            }
        }
    }
}

impl<'ast> Visit<'ast> for DatabaseFunctionScanner<'_> {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.record_path(path);
        visit::visit_path(self, path);
    }

    fn visit_lit_str(&mut self, literal: &'ast LitStr) {
        self.record_sql(literal);
    }

    fn visit_expr_method_call(&mut self, expression: &'ast syn::ExprMethodCall) {
        if matches!(
            expression.method.to_string().as_str(),
            "exclusive_transaction" | "immediate_transaction" | "transaction"
        ) {
            let LineColumn { line, .. } = expression.method.span().start();
            self.first_transaction_line.get_or_insert(line);
        }
        visit::visit_expr_method_call(self, expression);
    }

    // A nested item is its own function boundary and must never inherit the outer function's
    // table set. The item walker scans it separately.
    fn visit_item_fn(&mut self, _function: &'ast syn::ItemFn) {}
}

fn sql_words(sql: &str) -> Vec<String> {
    sql.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn sql_table_after(
    words: &[String],
    start: usize,
    schema_tables: &HashSet<String>,
) -> Option<String> {
    words
        .iter()
        .skip(start)
        .take(3)
        .find(|candidate| schema_tables.contains(candidate.as_str()))
        .cloned()
}

fn is_read_model_module(file: &str, allowlist: &[&str]) -> bool {
    allowlist.contains(&file)
}

struct DatabaseScanContext<'a> {
    file: &'a str,
    aliases: &'a HashMap<String, String>,
    schema_tables: &'a HashSet<String>,
    read_model: bool,
}

fn scan_database_function(
    context: &DatabaseScanContext<'_>,
    function_name: &str,
    function_line: usize,
    block: &syn::Block,
    violations: &mut Vec<Violation>,
) {
    let mut scanner = DatabaseFunctionScanner::new(context.aliases, context.schema_tables);
    scanner.visit_block(block);
    let tables = scanner.tables.into_iter().collect::<Vec<_>>();
    if context.read_model {
        if let Some(line) = scanner.first_write_line {
            violations.push(Violation {
                file: context.file.to_owned(),
                line,
                symbol: function_name.to_owned(),
                tables: tables.clone(),
                rule: ViolationRule::ReadModelWrite,
            });
        }
    } else if tables.len() > 1 {
        violations.push(Violation {
            file: context.file.to_owned(),
            line: function_line,
            symbol: function_name.to_owned(),
            tables: tables.clone(),
            rule: ViolationRule::MultipleDatabaseTables,
        });
    }
    if context.file != "server/src/db.rs"
        && let Some(line) = scanner.first_transaction_line
    {
        violations.push(Violation {
            file: context.file.to_owned(),
            line,
            symbol: function_name.to_owned(),
            tables: Vec::new(),
            rule: ViolationRule::TransactionOpening,
        });
    }
}

fn scan_database_items(
    context: &DatabaseScanContext<'_>,
    items: &[Item],
    violations: &mut Vec<Violation>,
) {
    for item in items {
        if disabled_in_production(item_attributes(item)) {
            continue;
        }
        match item {
            Item::Fn(function) => {
                let LineColumn { line, .. } = function.sig.ident.span().start();
                scan_database_function(
                    context,
                    &function.sig.ident.to_string(),
                    line,
                    &function.block,
                    violations,
                );
            }
            Item::Impl(item_impl) => {
                for implementation_item in &item_impl.items {
                    let syn::ImplItem::Fn(function) = implementation_item else {
                        continue;
                    };
                    if disabled_in_production(&function.attrs) {
                        continue;
                    }
                    let LineColumn { line, .. } = function.sig.ident.span().start();
                    scan_database_function(
                        context,
                        &function.sig.ident.to_string(),
                        line,
                        &function.block,
                        violations,
                    );
                }
            }
            Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    scan_database_items(context, nested, violations);
                }
            }
            _ => {}
        }
    }
}

fn scan_database_boundary_source(
    file: &str,
    source: &str,
    schema_tables: &HashSet<String>,
    read_model_modules: &[&str],
) -> Result<Vec<Violation>, syn::Error> {
    let syntax = syn::parse_file(source)?;
    let aliases = schema_aliases(&syntax, schema_tables);
    let mut violations = Vec::new();
    let context = DatabaseScanContext {
        file,
        aliases: &aliases,
        schema_tables,
        read_model: is_read_model_module(file, read_model_modules),
    };
    scan_database_items(&context, &syntax.items, &mut violations);
    Ok(violations)
}

fn database_boundary_violation_is_allowed(
    violation: &Violation,
    allowlist: &[DatabaseBoundaryAllowance],
) -> bool {
    allowlist.iter().any(|allowance| {
        allowance.file == violation.file
            && allowance.function == violation.symbol
            && allowance.rule == violation.rule
            && allowance
                .tables
                .iter()
                .copied()
                .eq(violation.tables.iter().map(String::as_str))
    })
}

fn module_directory(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    if path.file_stem().is_some_and(|stem| stem == "mod") {
        parent.to_path_buf()
    } else {
        path.file_stem()
            .map_or_else(|| parent.to_path_buf(), |stem| parent.join(stem))
    }
}

fn collect_test_only_module_paths(
    root: &Path,
    files: &[PathBuf],
) -> Result<HashSet<PathBuf>, ScanError> {
    let mut test_only = HashSet::new();
    for relative in files {
        let source =
            fs::read_to_string(root.join(relative)).map_err(|source| ScanError::ReadSource {
                path: relative.clone(),
                source,
            })?;
        let syntax = syn::parse_file(&source).map_err(|source| ScanError::ParseSource {
            path: relative.clone(),
            source,
        })?;
        collect_test_only_modules(
            root,
            &syntax.items,
            &module_directory(relative),
            &mut test_only,
        );
    }
    Ok(test_only)
}

fn collect_test_only_modules(
    root: &Path,
    items: &[Item],
    module_directory: &Path,
    test_only: &mut HashSet<PathBuf>,
) {
    for item in items {
        let Item::Mod(module) = item else {
            continue;
        };
        let child_directory = module_directory.join(module.ident.to_string());
        if let Some((_, nested)) = &module.content {
            if !disabled_in_production(&module.attrs) {
                collect_test_only_modules(root, nested, &child_directory, test_only);
            }
            continue;
        }
        if !disabled_in_production(&module.attrs) {
            continue;
        }
        let flat = module_directory.join(format!("{}.rs", module.ident));
        let nested = child_directory.join("mod.rs");
        if root.join(&flat).is_file() {
            test_only.insert(flat);
        } else if root.join(&nested).is_file() {
            test_only.insert(nested);
        }
    }
}

fn scan_database_boundary_rule(
    root: &Path,
    files: &[PathBuf],
    schema_tables: &HashSet<String>,
) -> Result<Vec<Violation>, ScanError> {
    let mut violations = Vec::new();
    let test_only_modules = collect_test_only_module_paths(root, files)?;
    for relative in files {
        if test_only_modules.contains(relative) {
            continue;
        }
        let source =
            fs::read_to_string(root.join(relative)).map_err(|source| ScanError::ReadSource {
                path: relative.clone(),
                source,
            })?;
        let file = relative.display().to_string();
        let scanned = scan_database_boundary_source(
            &file,
            &source,
            schema_tables,
            READ_MODEL_MODULE_ALLOWLIST,
        )
        .map_err(|source| ScanError::ParseSource {
            path: relative.clone(),
            source,
        })?;
        violations.extend(scanned.into_iter().filter(|violation| {
            !database_boundary_violation_is_allowed(violation, DATABASE_BOUNDARY_ALLOWLIST)
        }));
    }
    Ok(violations)
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

fn scan_production_symbol_rule(
    root: &Path,
    files: &[PathBuf],
    test_only_modules: &HashSet<PathBuf>,
    forbidden: &HashSet<&'static str>,
) -> Result<Vec<Violation>, ScanError> {
    let production_files = files
        .iter()
        .filter(|relative| !test_only_modules.contains(*relative))
        .cloned()
        .collect::<Vec<_>>();
    if production_files.is_empty() {
        return Err(ScanError::RuleSourcesMissing);
    }
    scan_rule(root, &production_files, forbidden)
}

fn scan_production_path_rule(
    root: &Path,
    files: &[PathBuf],
    test_only_modules: &HashSet<PathBuf>,
    forbidden_path: &[&'static str],
) -> Result<Vec<Violation>, ScanError> {
    let production_files = files
        .iter()
        .filter(|relative| !test_only_modules.contains(*relative))
        .collect::<Vec<_>>();
    if production_files.is_empty() {
        return Err(ScanError::RuleSourcesMissing);
    }

    let mut violations = Vec::new();
    for relative in production_files {
        let source =
            fs::read_to_string(root.join(relative)).map_err(|source| ScanError::ReadSource {
                path: relative.clone(),
                source,
            })?;
        violations.extend(
            scan_production_path_source(&relative.display().to_string(), &source, forbidden_path)
                .map_err(|source| ScanError::ParseSource {
                path: relative.clone(),
                source,
            })?,
        );
    }
    Ok(violations)
}

fn scan_public_visibility_rule(
    root: &Path,
    files: &[PathBuf],
    allowlist: &[PublicVisibilityAllowance],
) -> Result<Vec<Violation>, ScanError> {
    let mut violations = Vec::new();
    for relative in files {
        let source =
            fs::read_to_string(root.join(relative)).map_err(|source| ScanError::ReadSource {
                path: relative.clone(),
                source,
            })?;
        let file = relative.display().to_string();
        violations.extend(
            scan_public_visibility_source(&file, &source, allowlist).map_err(|source| {
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

fn require_empty_transition_allowlists() -> Result<(), ScanError> {
    if !DATABASE_BOUNDARY_ALLOWLIST.is_empty() {
        return Err(ScanError::CanaryFailed);
    }
    Ok(())
}

fn run_application_wire_canaries() -> Result<(), ScanError> {
    let application_wire_symbols = forbidden(APPLICATION_WIRE_FORBIDDEN);
    let application_wire_violations = scan_source(
        "<application-wire-canary>",
        APPLICATION_WIRE_CANARY,
        &application_wire_symbols,
    )
    .map_err(|_| ScanError::CanaryFailed)?;
    for symbol in application_wire_symbols {
        if !application_wire_violations
            .iter()
            .any(|violation| violation.symbol == symbol)
        {
            return Err(ScanError::CanaryFailed);
        }
    }
    if !scan_source(
        "<application-domain-protocol-canary>",
        APPLICATION_DOMAIN_PROTOCOL_CANARY,
        &forbidden(APPLICATION_WIRE_FORBIDDEN),
    )
    .map_err(|_| ScanError::CanaryFailed)?
    .is_empty()
    {
        return Err(ScanError::CanaryFailed);
    }
    Ok(())
}

fn run_shared_adapter_error_canaries() -> Result<(), ScanError> {
    for (source, symbols) in [
        (CONTEST_ADAPTER_ERROR_CANARY, CONTEST_ADAPTER_CALLER_ERRORS),
        (AUDIT_ADAPTER_ERROR_CANARY, AUDIT_ADAPTER_CALLER_ERRORS),
        (
            ENROLLMENT_REQUEST_ADAPTER_ERROR_CANARY,
            ENROLLMENT_REQUEST_ADAPTER_CALLER_ERRORS,
        ),
        (
            PROVISIONING_ADAPTER_ERROR_CANARY,
            PROVISIONING_ADAPTER_CALLER_ERRORS,
        ),
    ] {
        let violations = scan_source("<shared-adapter-error-canary>", source, &forbidden(symbols))
            .map_err(|_| ScanError::CanaryFailed)?;
        if violations.len() != symbols.len()
            || symbols.iter().any(|symbol| {
                !violations
                    .iter()
                    .any(|violation| violation.symbol == *symbol)
            })
        {
            return Err(ScanError::CanaryFailed);
        }
    }
    Ok(())
}

fn run_contest_adapter_ownership_canary() -> Result<(), ScanError> {
    let violations = scan_production_path_source(
        "<contest-adapter-import-path-canary>",
        CONTEST_ADAPTER_IMPORT_PATH_CANARY,
        CONTEST_ADAPTER_IMPORT_PATH,
    )
    .map_err(|_| ScanError::CanaryFailed)?;
    if violations.len() != 2
        || violations.iter().any(|violation| {
            violation.rule != ViolationRule::ForbiddenProductionPath
                || violation.symbol != CONTEST_ADAPTER_IMPORT_PATH.join("::")
        })
    {
        return Err(ScanError::CanaryFailed);
    }
    Ok(())
}

fn run_dependency_canaries() -> Result<(), ScanError> {
    run_application_wire_canaries()?;
    run_shared_adapter_error_canaries()?;
    run_contest_adapter_ownership_canary()
}

fn run_canaries() -> Result<(), ScanError> {
    require_empty_transition_allowlists()?;

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

    run_dependency_canaries()?;

    let public_visibility_violations =
        scan_public_visibility_source("<visibility-canary>", PUBLIC_VISIBILITY_CANARY, &[])
            .map_err(|_| ScanError::CanaryFailed)?;
    if public_visibility_violations.len() != 2
        || !public_visibility_violations
            .iter()
            .any(|violation| violation.symbol == "struct PublicItem")
        || !public_visibility_violations
            .iter()
            .any(|violation| violation.symbol == "mod public_module")
    {
        return Err(ScanError::CanaryFailed);
    }

    let schema_tables = ["audit_events", "commands", "devices"]
        .into_iter()
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    for source in [
        DB_DIRECT_TABLE_CANARY,
        DB_ALIAS_TABLE_CANARY,
        DB_SQL_JOIN_CANARY,
    ] {
        let violations =
            scan_database_boundary_source("server/src/db/canary.rs", source, &schema_tables, &[])
                .map_err(|_| ScanError::CanaryFailed)?;
        if violations.len() != 1
            || violations[0].rule != ViolationRule::MultipleDatabaseTables
            || violations[0].tables != ["commands", "devices"]
        {
            return Err(ScanError::CanaryFailed);
        }
    }
    if !scan_database_boundary_source(
        "server/src/db/canary.rs",
        DB_CFG_TEST_CANARY,
        &schema_tables,
        &[],
    )
    .map_err(|_| ScanError::CanaryFailed)?
    .is_empty()
    {
        return Err(ScanError::CanaryFailed);
    }
    let read_model_file = "server/src/db/query.rs";
    if !scan_database_boundary_source(
        read_model_file,
        DB_SQL_JOIN_CANARY,
        &schema_tables,
        &[read_model_file],
    )
    .map_err(|_| ScanError::CanaryFailed)?
    .is_empty()
    {
        return Err(ScanError::CanaryFailed);
    }
    let read_model_write = scan_database_boundary_source(
        read_model_file,
        DB_READ_MODEL_WRITE_CANARY,
        &schema_tables,
        &[read_model_file],
    )
    .map_err(|_| ScanError::CanaryFailed)?;
    if read_model_write.len() != 1 || read_model_write[0].rule != ViolationRule::ReadModelWrite {
        return Err(ScanError::CanaryFailed);
    }
    let transaction = scan_database_boundary_source(
        "server/src/db/canary.rs",
        DB_TRANSACTION_CANARY,
        &schema_tables,
        &[],
    )
    .map_err(|_| ScanError::CanaryFailed)?;
    if transaction.len() != 1 || transaction[0].rule != ViolationRule::TransactionOpening {
        return Err(ScanError::CanaryFailed);
    }
    Ok(())
}

fn run() -> Result<Vec<Violation>, ScanError> {
    run_canaries()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");

    let mut http_files = rust_files(&root, Path::new("server/src/http.rs"))?;
    http_files.extend(rust_files(&root, Path::new("server/src/http"))?);
    http_files.retain(|file| file.file_name().is_none_or(|name| name != "tests.rs"));

    let mut audit_files = rust_files(&root, Path::new("server/src/audit.rs"))?;
    audit_files.extend(rust_files(&root, Path::new("server/src/audit"))?);
    let mut audit_vault_files = audit_files.clone();
    audit_vault_files.extend(rust_files(&root, Path::new("server/src/vault.rs"))?);
    audit_vault_files.extend(rust_files(&root, Path::new("server/src/vault"))?);

    let mut application_files = rust_files(&root, Path::new("server/src/application.rs"))?;
    application_files.extend(rust_files(&root, Path::new("server/src/application"))?);
    let mut db_files = rust_files(&root, Path::new("server/src/db.rs"))?;
    db_files.extend(rust_files(&root, Path::new("server/src/db"))?);
    let test_only_db_modules = collect_test_only_module_paths(&root, &db_files)?;
    let schema_tables = schema_table_names(&root)?;
    let mut visibility_files = application_files.clone();
    visibility_files.extend(db_files.iter().cloned());
    visibility_files.extend(http_files.iter().cloned());
    visibility_files.extend(audit_files);

    if http_files.is_empty()
        || audit_vault_files.is_empty()
        || application_files.is_empty()
        || db_files.is_empty()
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
            "CommandState",
            "CommandStatus",
            "ControlEnvelope",
            "command::Body",
            "control_envelope",
            "diesel",
            "generated",
            "natsume_error_code",
            "pool",
            "prost",
            "r2d2",
            "sqlx",
            "utoipa",
        ]),
    )?);
    violations.extend(scan_public_visibility_rule(
        &root,
        &visibility_files,
        PUBLIC_VISIBILITY_ALLOWLIST,
    )?);
    violations.extend(scan_database_boundary_rule(
        &root,
        &db_files,
        &schema_tables,
    )?);
    let mut contest_adapter_files = rust_files(&root, Path::new("server/src/db/contest.rs"))?;
    contest_adapter_files.extend(rust_files(&root, Path::new("server/src/db/contest"))?);
    violations.extend(scan_production_symbol_rule(
        &root,
        &contest_adapter_files,
        &test_only_db_modules,
        &forbidden(CONTEST_ADAPTER_CALLER_ERRORS),
    )?);
    violations.extend(scan_production_path_rule(
        &root,
        &contest_adapter_files,
        &test_only_db_modules,
        CONTEST_ADAPTER_IMPORT_PATH,
    )?);
    violations.extend(scan_production_symbol_rule(
        &root,
        &rust_files(&root, Path::new("server/src/db/audit.rs"))?,
        &test_only_db_modules,
        &forbidden(AUDIT_ADAPTER_CALLER_ERRORS),
    )?);
    violations.extend(scan_production_symbol_rule(
        &root,
        &rust_files(
            &root,
            Path::new("server/src/db/device/enrollment/request.rs"),
        )?,
        &test_only_db_modules,
        &forbidden(ENROLLMENT_REQUEST_ADAPTER_CALLER_ERRORS),
    )?);
    violations.extend(scan_production_symbol_rule(
        &root,
        &rust_files(&root, Path::new("server/src/db/provisioning.rs"))?,
        &test_only_db_modules,
        &forbidden(PROVISIONING_ADAPTER_CALLER_ERRORS),
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

    #[test]
    fn bare_public_visibility_is_forbidden_but_restricted_visibility_is_allowed() {
        let source = "pub struct PublicItem;
pub mod public_module {}
pub(crate) fn crate_item() {}
pub(super) const PARENT_ITEM: usize = 1;
pub(in crate) type ScopedItem = usize;
fn private_item() {}
";
        let violations = match scan_public_visibility_source("fixture.rs", source, &[]) {
            Ok(violations) => violations,
            Err(error) => panic!("fixture did not parse: {error}"),
        };
        assert_eq!(violations.len(), 2);
        assert!(
            violations
                .iter()
                .all(|violation| matches!(violation.rule, ViolationRule::BarePublicVisibility))
        );
    }

    #[test]
    fn public_visibility_allowlist_is_exact() {
        let source = "pub struct Allowed;
pub struct Rejected;
";
        let allowlist = [PublicVisibilityAllowance {
            file: "fixture.rs",
            item_kind: "struct",
            item_name: "Allowed",
        }];
        let violations = match scan_public_visibility_source("fixture.rs", source, &allowlist) {
            Ok(violations) => violations,
            Err(error) => panic!("fixture did not parse: {error}"),
        };
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].symbol, "struct Rejected");
    }
}
