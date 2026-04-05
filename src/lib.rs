use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::Path;

use darling::{ast::NestedMeta, FromMeta};
use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use proc_macro::TokenStream;
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use syn::{parse_macro_input, visit::Visit, ItemFn};

/// Configuration resolved via figment (defaults → slop.toml → SLOP_* env).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlopConfig {
    model: String,
    retries: usize,
    provider: String,
    api_key_env: String,
    run_doctests: bool,
}

impl Default for SlopConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4o-mini".into(),
            retries: 5,
            provider: "https://openrouter.ai/api/v1/chat/completions".into(),
            api_key_env: "OPEN_ROUTER_API_KEY".into(),
            run_doctests: false,
        }
    }
}

/// Loads config from defaults → slop.toml → SLOP_* env vars.
fn load_config(manifest_dir: &str) -> SlopConfig {
    Figment::new()
        .merge(Serialized::defaults(SlopConfig::default()))
        .merge(Toml::file(Path::new(manifest_dir).join("slop.toml")))
        .merge(Env::prefixed("SLOP_").lowercase(true))
        .extract()
        .unwrap_or_default()
}

/// Parsed `#[slop(...)]` attribute arguments — highest priority overrides.
#[derive(Debug, FromMeta)]
struct SlopArgs {
    #[darling(default)]
    model: Option<String>,
    #[darling(default)]
    retries: Option<usize>,
    #[darling(default)]
    provider: Option<String>,
    #[darling(default)]
    api_key_env: Option<String>,
    #[darling(default)]
    nocache: bool,
    #[darling(default)]
    run_doctests: Option<bool>,
    #[darling(default)]
    dump: Option<String>,
    #[darling(default)]
    context_file: Option<String>,
    #[darling(default)]
    hint: Option<String>,
}

/// Merges attribute args on top of figment config.
struct ResolvedConfig {
    model: String,
    retries: usize,
    provider: String,
    api_key_env: String,
    run_doctests: bool,
}

impl ResolvedConfig {
    fn from(cfg: SlopConfig, args: &SlopArgs) -> Self {
        Self {
            model: args.model.clone().unwrap_or(cfg.model),
            retries: args.retries.unwrap_or(cfg.retries),
            provider: args.provider.clone().unwrap_or(cfg.provider),
            api_key_env: args.api_key_env.clone().unwrap_or(cfg.api_key_env),
            run_doctests: args.run_doctests.unwrap_or(cfg.run_doctests),
        }
    }
}

#[proc_macro_attribute]
pub fn slop(attr: TokenStream, item: TokenStream) -> TokenStream {
    let original = item.clone();
    let attr_args = match NestedMeta::parse_meta_list(attr.into()) {
        Ok(a) => a,
        Err(e) => return TokenStream::from(darling::Error::from(e).write_errors()),
    };
    let func = parse_macro_input!(item as ItemFn);

    let args = match SlopArgs::from_list(&attr_args) {
        Ok(a) => a,
        Err(e) => return e.write_errors().into(),
    };

    // Load .env — walk from CARGO_MANIFEST_DIR upward so workspace-root
    // .env files are found even when compiling a sub-crate.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    {
        let mut dir = std::path::PathBuf::from(&manifest_dir);
        loop {
            let candidate = dir.join(".env");
            if candidate.is_file() {
                let _ = dotenvy::from_path(&candidate);
                break;
            }
            if !dir.pop() {
                break;
            }
        }
    }
    let cfg = load_config(&manifest_dir);
    let rc = ResolvedConfig::from(cfg, &args);

    let source = func.to_token_stream().to_string();
    let pretty_source = prettify_source(&func);
    let fn_name = func.sig.ident.to_string();
    let doctests = extract_doctests(&func);
    let doc_attrs = extract_doc_attrs(&func);

    if doctests.is_some() {
        eprintln!("  {:>12} {fn_name} (found doctests)", "Slop");
    }

    let cache_key = compute_cache_key(&source, &rc.model, &rc.provider);
    let cache_file = Path::new(&manifest_dir)
        .join("target")
        .join("slop-cache")
        .join(format!("{fn_name}_{cache_key:016x}.rs"));
    let tag = format!("{fn_name}_{cache_key:016x}");

    if !args.nocache {
        if let Ok(cached) = std::fs::read_to_string(&cache_file) {
            if let Ok(parsed) = syn::parse_str::<ItemFn>(&cached) {
                eprintln!("  {:>12} {fn_name} (cached)", "Slop");
                let out = prepend_doc_attrs(&doc_attrs, &parsed);
                maybe_dump(&args.dump, &manifest_dir, &fn_name, &cached);
                return out.into();
            }
            eprintln!("  {:>12} {fn_name} (cache corrupt, regenerating)", "Slop");
        }
    }

    let api_key = match std::env::var(&rc.api_key_env) {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("  {:>12} {fn_name} ({} not set, keeping stub)", "Slop", rc.api_key_env);
            return original;
        }
    };

    let deps_ctx = collect_deps(&manifest_dir);
    let file_ctx = args.context_file.as_ref().map(|p| {
        let path = Path::new(&manifest_dir).join(p);
        std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("  {:>12} {fn_name} (failed to read context file: {e})", "Warning");
            String::new()
        })
    });

    // Auto-resolve type definitions from the signature.
    let sig_types = extract_signature_types(&func);
    let type_ctx = resolve_types_from_source(&manifest_dir, &sig_types);
    if !type_ctx.is_empty() {
        eprintln!("  {:>12} {fn_name} (resolved {} type(s) from signature)", "Slop", sig_types.len());
    }

    let system = build_system_prompt(&deps_ctx);
    let user = build_user_prompt(&pretty_source, file_ctx.as_deref(), &type_ctx, args.hint.as_deref());

    let mut messages = vec![
        serde_json::json!({"role": "system", "content": system}),
        serde_json::json!({"role": "user", "content": user}),
    ];

    let mut last_parseable: Option<proc_macro2::TokenStream> = None;

    for attempt in 1..=rc.retries {
        let content = match call_llm(&api_key, &rc.provider, &rc.model, &messages) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  {:>12} {fn_name} (LLM error: {e})", "Error");
                break;
            }
        };

        let code = strip_fences(&content);

        let parsed = match syn::parse_str::<ItemFn>(&code) {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "  {:>12} {fn_name} (parse error, attempt {attempt}/{retries})",
                    "Retrying",
                    retries = rc.retries
                );
                messages.push(serde_json::json!({"role": "assistant", "content": &content}));
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": format!(
                        "Syntax error:\n\n{e}\n\n\
                         Fix the function. Return ONLY the complete corrected function."
                    )
                }));
                continue;
            }
        };

        last_parseable = Some(parsed.to_token_stream());

        // Lint for panic-prone patterns before compiling.
        if let Err(lint_errors) = lint_panics(&code) {
            eprintln!(
                "  {:>12} {fn_name} (lint errors, attempt {attempt}/{retries})",
                "Retrying",
                retries = rc.retries
            );
            let fix_msg = format!(
                "{lint_errors}\n\n\
                 NEVER use .unwrap(), .expect(), or panic!(). \
                 Use pattern matching, .unwrap_or(), or .unwrap_or_default() instead.\n\n\
                 Fix the function. Return ONLY the complete corrected function."
            );
            messages.push(serde_json::json!({"role": "assistant", "content": &content}));
            messages.push(serde_json::json!({"role": "user", "content": fix_msg}));
            continue;
        }

        // Prepend auto-resolved type defs so rustc can verify the function.
        let check_code = if type_ctx.is_empty() {
            code.clone()
        } else {
            format!("{type_ctx}\n\n{code}")
        };

        match try_rustc(&check_code, &tag) {
            Ok(()) => {}
            Err(errors) => {
                eprintln!(
                    "  {:>12} {fn_name} (compile errors, attempt {attempt}/{retries})",
                    "Retrying",
                    retries = rc.retries
                );
                let mut fix_msg = format!("The code has compile errors:\n\n{errors}\n\n");
                if !type_ctx.is_empty() {
                    fix_msg.push_str(
                        "Remember: these types are already in scope (use them directly, \
                         no module paths):\n\n",
                    );
                    fix_msg.push_str(&type_ctx);
                    fix_msg.push_str("\n\n");
                }
                fix_msg.push_str("Fix the function. Return ONLY the complete corrected function.");
                messages.push(serde_json::json!({"role": "assistant", "content": &content}));
                messages.push(serde_json::json!({"role": "user", "content": fix_msg}));
                continue;
            }
        }

        // Run doctests if present and enabled, verify behavior, not just types.
        if rc.run_doctests {
            if let Some(ref tests) = doctests {
                match try_run(&check_code, tests, &tag) {
                    Ok(()) => {
                        eprintln!(
                            "  {:>12} {fn_name} (attempt {attempt}/{retries}, doctests passed)",
                            "Slop",
                            retries = rc.retries
                        );
                    }
                    Err(runtime_err) => {
                        eprintln!(
                            "  {:>12} {fn_name} (doctest failed, attempt {attempt}/{retries})",
                            "Retrying",
                            retries = rc.retries
                        );
                        let fix_msg = format!(
                            "The code compiles but fails at runtime. The following doctests failed:\n\n\
                             ```\n{tests}\n```\n\n\
                             Runtime error:\n\n{runtime_err}\n\n\
                             Fix the function so it passes these tests. \
                             Return ONLY the complete corrected function."
                        );
                        messages.push(serde_json::json!({"role": "assistant", "content": &content}));
                        messages.push(serde_json::json!({"role": "user", "content": fix_msg}));
                        continue;
                    }
                }
            } else {
                eprintln!("  {:>12} {fn_name} (attempt {attempt}/{retries})", "Slop", retries = rc.retries);
            }
        } else {
            eprintln!("  {:>12} {fn_name} (attempt {attempt}/{retries})", "Slop", retries = rc.retries);
        }

        write_cache(&cache_file, &code, &system, &user);
        maybe_dump(&args.dump, &manifest_dir, &fn_name, &code);
        let out = prepend_doc_attrs(&doc_attrs, &parsed);
        return out.into();
    }

    if let Some(tokens) = last_parseable {
        eprintln!("  {:>12} {fn_name} (retries exhausted, using last parseable attempt)", "Warning");
        let mut out = doc_attrs.clone();
        out.extend(quote::quote!(#[allow(clippy::all)]));
        out.extend(tokens);
        out.into()
    } else {
        eprintln!("  {:>12} {fn_name} (retries exhausted, keeping stub)", "Warning");
        original
    }
}

// Known primitives/std types we don't need to resolve.
const BUILTIN_TYPES: &[&str] = &[
    "bool",
    "char",
    "f32",
    "f64",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "str",
    "String",
    "Vec",
    "Option",
    "Result",
    "Box",
    "Rc",
    "Arc",
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
    "Cow",
    "Pin",
    "PhantomData",
    "Self",
];

/// Walks a function signature's types and collects non-builtin type names.
struct TypeCollector {
    names: HashSet<String>,
}

impl TypeCollector {
    fn new() -> Self {
        Self { names: HashSet::new() }
    }
}

impl<'ast> Visit<'ast> for TypeCollector {
    fn visit_path_segment(&mut self, seg: &'ast syn::PathSegment) {
        let name = seg.ident.to_string();
        if !BUILTIN_TYPES.contains(&name.as_str()) {
            self.names.insert(name);
        }
        syn::visit::visit_path_segment(self, seg);
    }
}

/// Converts `#[doc = "..."]` attributes back to `/// ...` for cleaner LLM prompts.
fn prettify_source(func: &ItemFn) -> String {
    let mut lines = Vec::new();
    for attr in &func.attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                    lines.push(format!("///{}", s.value()));
                }
            }
        }
    }
    // Build a copy of the function without doc attrs for the signature + body.
    let mut bare = func.clone();
    bare.attrs.retain(|a| !a.path().is_ident("doc"));
    let sig_body = bare.to_token_stream().to_string();
    if lines.is_empty() {
        sig_body
    } else {
        lines.push(sig_body);
        lines.join("\n")
    }
}

/// Extracts doc attributes as a token stream to prepend to generated output.
fn extract_doc_attrs(func: &ItemFn) -> proc_macro2::TokenStream {
    let mut tokens = proc_macro2::TokenStream::new();
    for attr in &func.attrs {
        if attr.path().is_ident("doc") {
            tokens.extend(attr.to_token_stream());
        }
    }
    tokens
}

/// Prepends doc attributes and `#[allow(clippy::all)]` to a generated function.
fn prepend_doc_attrs(doc_attrs: &proc_macro2::TokenStream, func: &ItemFn) -> proc_macro2::TokenStream {
    let mut out = doc_attrs.clone();
    out.extend(quote::quote!(#[allow(clippy::all)]));
    out.extend(func.to_token_stream());
    out
}

/// Extracts non-primitive type names referenced in a function signature.
fn extract_signature_types(func: &ItemFn) -> HashSet<String> {
    let mut collector = TypeCollector::new();
    collector.visit_signature(&func.sig);
    collector.names
}

/// Scans `src/**/*.rs` for struct/enum/type/trait definitions matching the given names.
/// Returns the source text of matching items.
fn resolve_types_from_source(manifest_dir: &str, type_names: &HashSet<String>) -> String {
    if type_names.is_empty() {
        return String::new();
    }

    let src_dir = Path::new(manifest_dir).join("src");
    let mut found = Vec::new();
    let mut found_names = HashSet::new();

    if let Ok(entries) = glob_rs_files(&src_dir) {
        for path in entries {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if let Ok(file) = syn::parse_file(&content) {
                for item in &file.items {
                    let (name, text) = match item {
                        syn::Item::Struct(s) => (s.ident.to_string(), item.to_token_stream().to_string()),
                        syn::Item::Enum(e) => (e.ident.to_string(), item.to_token_stream().to_string()),
                        syn::Item::Type(t) => (t.ident.to_string(), item.to_token_stream().to_string()),
                        syn::Item::Trait(t) => (t.ident.to_string(), item.to_token_stream().to_string()),
                        _ => continue,
                    };

                    if type_names.contains(&name) && found_names.insert(name.clone()) {
                        found.push(text);
                    }
                }
            }
        }
    }

    found.join("\n\n")
}

/// Recursively collects all `.rs` files under a directory.
fn glob_rs_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    if !dir.is_dir() {
        return Ok(files);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            files.extend(glob_rs_files(&path)?);
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    Ok(files)
}

fn build_system_prompt(deps: &str) -> String {
    let mut prompt = String::from(
        "You are a Rust expert code generator. You receive a Rust function with \
         doc comments and a stub body. Generate a correct, compiling implementation. \
         Return ONLY the complete function definition (including `fn`, signature, and \
         body). No markdown fences, no explanation, no imports outside the function. \
         Any type definitions shown in the prompt are already in scope — use them \
         directly without module paths or `use` statements. \
         NEVER use .unwrap(), .expect(), panic!(), todo!(), or unimplemented!() — handle errors gracefully \
         with pattern matching, .unwrap_or(), .unwrap_or_default(), or early returns. \
         If the doc comments contain ``` code blocks with assertions, those will be \
         compiled and run as tests — the function MUST pass them. Make no Mistakes.",
    );

    if !deps.is_empty() {
        prompt.push_str("\n\nThe crate has these dependencies available:\n");
        prompt.push_str(deps);
    }

    prompt
}

fn build_user_prompt(source: &str, context_file: Option<&str>, type_context: &str, hint: Option<&str>) -> String {
    let mut prompt = format!("Implement this Rust function. The doc comments and body are context hints:\n\n{source}");

    if !type_context.is_empty() {
        prompt.push_str("\n\nThe following type definitions are used in the signature:\n\n");
        prompt.push_str(type_context);
    }

    if let Some(ctx) = context_file {
        if !ctx.is_empty() {
            prompt.push_str("\n\nHere are additional type definitions and context from the project:\n\n");
            prompt.push_str(ctx);
        }
    }

    if let Some(h) = hint {
        prompt.push_str("\n\nHint: ");
        prompt.push_str(h);
    }

    prompt
}

/// Reads `Cargo.toml` from `manifest_dir` and extracts dependencies with features.
fn collect_deps(manifest_dir: &str) -> String {
    let path = Path::new(manifest_dir).join("Cargo.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let table: toml::Table = match content.parse() {
        Ok(t) => t,
        Err(_) => return String::new(),
    };

    let mut deps = Vec::new();
    for key in ["dependencies", "dev-dependencies"] {
        if let Some(toml::Value::Table(dep_table)) = table.get(key) {
            for (name, value) in dep_table {
                let features = match value {
                    toml::Value::Table(t) => t
                        .get("features")
                        .and_then(|f| f.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")),
                    _ => None,
                };

                let entry = match features {
                    Some(f) if !f.is_empty() => format!("{name} (features: {f})"),
                    _ => name.clone(),
                };

                if !deps.iter().any(|d: &String| d.starts_with(name.as_str())) {
                    deps.push(entry);
                }
            }
        }
    }
    deps.sort();
    deps.join(", ")
}

fn compute_cache_key(source: &str, model: &str, provider: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    model.hash(&mut hasher);
    provider.hash(&mut hasher);
    hasher.finish()
}

fn write_cache(path: &Path, code: &str, system: &str, user: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut out = String::new();
    out.push_str("// ── system prompt ──\n");
    for line in system.lines() {
        out.push_str("// ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("//\n// ── user prompt ──\n");
    for line in user.lines() {
        out.push_str("// ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("//\n// ── generated ──\n\n");
    out.push_str(code);
    out.push('\n');
    let tmp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&tmp, out) {
        eprintln!("  {:>12} failed to write cache: {e}", "Warning");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        eprintln!("  {:>12} failed to write cache: {e}", "Warning");
    }
}

fn maybe_dump(dump: &Option<String>, manifest_dir: &str, fn_name: &str, code: &str) {
    if let Some(dump_path) = dump {
        let path = Path::new(manifest_dir).join(dump_path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, code) {
            eprintln!("  {:>12} {fn_name} (failed to dump to {}: {e})", "Warning", path.display());
        } else {
            eprintln!("  {:>12} {fn_name} to {}", "Dumped", path.display());
        }
    }
}

fn call_llm(api_key: &str, url: &str, model: &str, messages: &[serde_json::Value]) -> Result<String, String> {
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": 0.2,
    });

    let resp = ureq::post(url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .send_json(&body)
        .map_err(|e| match e {
            ureq::Error::Status(code, resp) => {
                let body = resp.into_string().unwrap_or_default();
                format!("HTTP {code}: {body}")
            }
            other => format!("Request error: {other}"),
        })?;

    let json: serde_json::Value = resp.into_json().map_err(|e| format!("JSON error: {e}"))?;
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "no content in response".into())
}

fn strip_fences(s: &str) -> String {
    let s = s.trim();
    let backtick_count = s.bytes().take_while(|&b| b == b'`').count();
    if backtick_count < 3 {
        return s.to_string();
    }
    let rest = &s[backtick_count..];
    // Strip language tag (e.g. "rust", "rs", "rust,no_run") up to the first newline.
    let rest = match rest.find('\n') {
        Some(nl) => &rest[nl + 1..],
        None => return s.to_string(),
    };
    let fence = &s[..backtick_count];
    let trimmed = rest.trim_end();
    if let Some(code) = trimmed.strip_suffix(fence) {
        return code.trim().to_string();
    }
    // Fallback: try stripping just 3 backticks.
    if backtick_count > 3 {
        if let Some(code) = trimmed.strip_suffix("```") {
            return code.trim().to_string();
        }
    }
    s.to_string()
}

/// Checks if `pattern` appears in `line` outside of string literals.
fn contains_outside_string(line: &str, pattern: &str) -> bool {
    let bytes = line.as_bytes();
    let pat = pattern.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        if in_string {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'"' {
            in_string = true;
            i += 1;
            continue;
        }
        if i + pat.len() <= bytes.len() && &bytes[i..i + pat.len()] == pat {
            return true;
        }
        i += 1;
    }
    false
}

/// Scans generated code for panic-prone patterns.
fn lint_panics(code: &str) -> Result<(), String> {
    let mut issues = Vec::new();
    for (i, line) in code.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("///") {
            continue;
        }
        if contains_outside_string(line, ".unwrap()") {
            issues.push(format!("  line {}: .unwrap() — use pattern matching or .unwrap_or() instead", i + 1));
        }
        if contains_outside_string(line, ".expect(") {
            issues.push(format!("  line {}: .expect() — use pattern matching or .unwrap_or() instead", i + 1));
        }
        if contains_outside_string(line, "panic!(") {
            issues.push(format!("  line {}: panic!() — handle the error gracefully instead", i + 1));
        }
        if contains_outside_string(line, "todo!(") {
            issues.push(format!("  line {}: todo!() — provide a real implementation", i + 1));
        }
        if contains_outside_string(line, "unimplemented!(") {
            issues.push(format!("  line {}: unimplemented!() — provide a real implementation", i + 1));
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(format!("Code contains panic-prone patterns:\n{}", issues.join("\n")))
    }
}

/// Extracts doctest code blocks from `/// ```...```` doc comments on a function.
fn extract_doctests(func: &ItemFn) -> Option<String> {
    let mut doc_lines = Vec::new();
    for attr in &func.attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                    doc_lines.push(s.value());
                }
            }
        }
    }

    let mut tests = Vec::new();
    let mut in_block = false;
    let mut block = Vec::new();

    for line in &doc_lines {
        let trimmed = line.trim();
        if !in_block {
            if trimmed == "```" || trimmed == "```rust" || trimmed == "```rs" {
                in_block = true;
                block.clear();
            }
        } else if trimmed == "```" {
            in_block = false;
            if !block.is_empty() {
                tests.push(block.join("\n"));
            }
        } else {
            // Strip leading space that doc comments typically have.
            let code_line = line.strip_prefix(' ').unwrap_or(line);
            block.push(code_line.to_string());
        }
    }

    if tests.is_empty() {
        None
    } else {
        Some(tests.join("\n"))
    }
}

/// Compiles the generated function + doctest assertions into a binary and runs it.
/// Returns Ok(()) if the binary exits successfully, Err with stderr on failure.
fn try_run(code: &str, doctests: &str, tag: &str) -> Result<(), String> {
    let dir = std::env::temp_dir().join("slopc");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let harness = format!("{code}\n\nfn main() {{\n{doctests}\n}}\n");
    let src = dir.join(format!("slop_doctest_{tag}.rs"));
    std::fs::write(&src, &harness).map_err(|e| e.to_string())?;

    let bin = dir.join(format!("slop_doctest_{tag}"));
    let compile = std::process::Command::new("rustc")
        .args(["--edition", "2021"])
        .arg("-o")
        .arg(&bin)
        .arg(&src)
        .output()
        .map_err(|e| format!("failed to run rustc: {e}"))?;

    if !compile.status.success() {
        return Err(format!("doctest compile error:\n{}", String::from_utf8_lossy(&compile.stderr)));
    }

    let run = std::process::Command::new(&bin)
        .output()
        .map_err(|e| format!("failed to run doctest binary: {e}"))?;

    if run.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&run.stderr).into_owned())
    }
}

fn try_rustc(code: &str, tag: &str) -> Result<(), String> {
    let dir = std::env::temp_dir().join("slopc");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let src = dir.join(format!("slop_check_{tag}.rs"));
    std::fs::write(&src, code).map_err(|e| e.to_string())?;

    let output = std::process::Command::new("rustc")
        .args(["--edition", "2021", "--crate-type", "lib", "--emit", "metadata"])
        .arg("-o")
        .arg(dir.join(format!("slop_check_{tag}.rmeta")))
        .arg(&src)
        .output()
        .map_err(|e| format!("failed to run rustc: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}
