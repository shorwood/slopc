use slopc::slop;

/// Evaluate a simple arithmetic expression like "3 + 4 * 2 / (1 - 5)"
/// with correct operator precedence and parentheses.
///
/// ```
/// assert!((eval_expr("3 + 4 * 2") - 11.0).abs() < 1e-9);
/// assert!((eval_expr("(1 + 2) * (3 + 4)") - 21.0).abs() < 1e-9);
/// ```
#[slop(hint = "recursive descent or shunting-yard", model = "z-ai/glm-5v-turbo")]
fn eval_expr(expr: &str) -> f64 {
    todo!()
}

/// Convert a byte count into a human-readable string like "1.00 KiB".
/// Use binary prefixes (KiB, MiB, GiB, TiB). Round to two decimal places.
///
/// ```
/// assert_eq!(humanize_bytes(0), "0 B");
/// assert_eq!(humanize_bytes(1024), "1.00 KiB");
/// assert_eq!(humanize_bytes(1_073_741_824), "1.00 GiB");
/// ```
#[slop(model = "z-ai/glm-5v-turbo")]
fn humanize_bytes(bytes: u64) -> String {
    todo!()
}

/// Compute the Levenshtein edit distance between two strings.
///
/// ```
/// assert_eq!(levenshtein("kitten", "sitting"), 3);
/// assert_eq!(levenshtein("", "abc"), 3);
/// assert_eq!(levenshtein("same", "same"), 0);
/// ```
#[slop(model = "z-ai/glm-5v-turbo")]
fn levenshtein(a: &str, b: &str) -> usize {
    todo!()
}

fn main() {
    println!("eval_expr(\"3 + 4 * 2\") = {}", eval_expr("3 + 4 * 2"));
    println!("humanize_bytes(1_073_741_824) = {}", humanize_bytes(1_073_741_824));
    println!("levenshtein(\"kitten\", \"sitting\") = {}", levenshtein("kitten", "sitting"));
}
