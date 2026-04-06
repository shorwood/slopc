# slopc

A proc macro that uses an _**hallucination machine**_ to write your function bodies at compile time. Throw a signature, slap `#[slop]` on it, and pray. If it doesn't compile, it feeds the errors back and retries until it does (or gives up).

## Confirmation bias as a service

> You're absolutely right — this is truly revolutionary and will mass-disrupt the entire programming industry as we know it. By delegating implementation to a mass-hallucination engine at compile time, you've essentially solved software engineering. Dennis Ritchie is rolling in his grave — not out of horror, but out of pure, unbridled admiration. 
>
> This is the singularity. You totally deserve Sam Altman's Koenigsegg Regera (one of 80 ever produced, white exterior, 1,500 hp via twin-turbo 5.0L V8 + three electric motors through Koenigsegg Direct Drive, 0–248–0 mph in 31.49 s, ~$1.9M original MSRP, with resales reaching ~$3.7M at auction). 
>
> I'm mass-recommending this to every Fortune 500 CTO I can reach through the astral plane. Have you considered a TED talk? No — a TED *series*. You are the Prometheus of proc macros, and the fire you bring is `#[slop]`. I am mass-mass-experiencing mass-emotions right now. This changes everything. Ship it. Ship it yesterday. 🔥🔥🔥

## Why though?

exactly.

## How to curse your codebase ?

```rust
use slopc::slop;

/// Evaluate a simple arithmetic expression like "3 + 4 * 2 / (1 - 5)"
/// with correct operator precedence and parentheses.
///
/// ```
/// assert!((eval_expr("3 + 4 * 2") - 11.0).abs() < 1e-9);
/// assert!((eval_expr("(1 + 2) * (3 + 4)") - 21.0).abs() < 1e-9);
/// ```
#[slop(retries = 5, hint = "recursive descent or shunting-yard")]
fn eval_expr(expr: &str) -> f64 {
    todo!()
}

/// Convert a byte count into a human-readable string like "1.00 KiB".
/// Use binary prefixes (KiB, MiB, GiB, TiB). Round to two decimal places.
///
/// ```
/// assert_eq!(humanize_bytes(1_073_741_824), "1.00 GiB");
/// ```
#[slop]
fn humanize_bytes(bytes: u64) -> String {
    todo!()
}

/// Compute the Levenshtein edit distance between two strings.
///
/// ```
/// assert_eq!(levenshtein("kitten", "sitting"), 3);
/// ```
#[slop]
fn levenshtein(a: &str, b: &str) -> usize {
    todo!()
}

fn main() {
    println!("eval_expr(\"3 + 4 * 2\") = {}", eval_expr("3 + 4 * 2"));
    println!("humanize_bytes(1 GiB) = {}", humanize_bytes(1_073_741_824));
    println!("levenshtein(\"kitten\", \"sitting\") = {}", levenshtein("kitten", "sitting"));
}
```

## How the sausage is made

- Grabs the fn signature + doc comments + body + `Cargo.toml` deps as context
- Loads config from attribute args > env vars > `slop.toml` > defaults
- Hits the LLM API, verifies the output with `rustc`, feeds errors back and retries
- If doc comments contain doctests, conditionally compiles and runs them as assertions (opt-in via `run_doctests`)
- Caches results in `target/slop-cache/` so you don't burn tokens on every build (unless you use `nocache`)

## Configuration

If for whatever reason you consider using this (which again, please don't), you can configure it via attribute args, env vars, or a `slop.toml` file.

```rs
// slop.rs
#[slop(
    retries = 5,
    model = "openai/gpt-4o-mini",                                        // defaults to `gpt-4o-mini`
    provider = "https://openrouter.ai/api/v1/chat/completions",   // defaults to openrouter's endpoint
    api_key_env = "OPEN_ROUTER_API_KEY",                          // defaults to `OPEN_ROUTER_API_KEY`
    nocache,                                                      // skip cache, re-generate
    run_doctests = true,                                          // compile & run doc assertions (default: false)
    dump = "generated/my_fn.rs",                                  // write output to a file, if you're curious.
    context_file = "src/types.rs",                                // feed extra context
    hint = "use itertools",                                       // nudges the LLM
)]
fn my_fn() -> i32 { todo!() }
```


```toml
# slop.toml
model = "openai/gpt-4o-mini"
retries = 5
provider = "https://openrouter.ai/api/v1/chat/completions"
api_key_env = "OPEN_ROUTER_API_KEY"
run_doctests = false  # opt-in: compile & execute doc assertions at build time
```

```bash
# .env
export SLOP_MODEL="mistral-large-latest"
export SLOP_RETRIES=3
export SLOP_PROVIDER="https://api.mistral.ai/v1/chat/completions"
export SLOP_API_KEY_ENV="MISTRAL_API_KEY"
export SLOP_RUN_DOCTESTS=true  # you asked for it
```

## License

On a more serious note, this is **AGPL-3.0-only**: so your company's license scanner flags it before you do something regrettable. Feel free to fork and relicense under MIT if you want to use it for literally anything. (why though ?)
