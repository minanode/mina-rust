//! Bounded subset of JSONPath for the node-thread state inspection RPC.

use jsonpath_rust::{
    parser::model::{JsonPath, JsonPathIndex},
    path::json_path_instance,
    JsonPathValue,
};
use mina_node::rpc::{RpcStateGetError, RpcStateGetResponse};
use serde_json::Value;

const MAX_FILTER_BYTES: usize = 1024;

fn error(message: &str) -> RpcStateGetError {
    RpcStateGetError::FilterError(message.to_owned())
}

// jsonpath-rust 0.5.0 has unchecked conversions in both parsing and evaluation
// (notably UnionIndex::from_indexes for [0,-1]). Only pure JSON work belongs in
// this boundary: never unwind through the node's mutable state or service.
fn guarded<T>(
    f: impl FnOnce() -> Result<T, RpcStateGetError> + std::panic::UnwindSafe,
) -> Result<T, RpcStateGetError> {
    std::panic::catch_unwind(f).unwrap_or_else(|_| Err(error("invalid or unsupported filter")))
}

pub(super) struct StateFilter(JsonPath);

impl StateFilter {
    pub(super) fn parse(filter: &str) -> Result<Self, RpcStateGetError> {
        if filter.len() > MAX_FILTER_BYTES {
            return Err(error("filter exceeds 1024 bytes"));
        }
        // Exclude recursive grammar before calling the library parser. These
        // reserved characters are unsupported even inside quoted field names.
        if !filter.starts_with('$') || filter.contains(['@', '?']) {
            return Err(error("expected a rooted field/index path"));
        }
        guarded(|| {
            let path = JsonPath::try_from(filter).map_err(|_| error("invalid filter"))?;
            let JsonPath::Chain(parts) = &path else {
                return Err(error("unsupported filter"));
            };
            for (i, part) in parts.iter().enumerate() {
                let supported = match part {
                    JsonPath::Root => i == 0,
                    JsonPath::Field(_) | JsonPath::Wildcard => true,
                    JsonPath::Index(JsonPathIndex::Single(index)) => index
                        .as_u64()
                        .and_then(|i| usize::try_from(i).ok())
                        .is_some(),
                    JsonPath::Index(JsonPathIndex::Slice(start, end, step)) => {
                        *start >= 0 && *end >= 0 && *step > 0
                    }
                    // Unions and recursive descent can multiply intermediate
                    // results; predicates can repeatedly scan the whole state.
                    _ => false,
                };
                if !supported {
                    return Err(error("unsupported filter operation"));
                }
            }
            Ok(Self(path))
        })
    }

    pub(super) fn evaluate(&self, value: &Value) -> RpcStateGetResponse {
        guarded(|| {
            let mut values = json_path_instance(&self.0, value, Default::default())
                .find(JsonPathValue::from_root(value))
                .into_iter()
                .filter(JsonPathValue::has_value)
                .map(JsonPathValue::to_data)
                .collect::<Vec<_>>();
            match values.len() {
                0 => Err(error("filter matched no value")),
                1 => Ok(values.remove(0)),
                _ => Ok(Value::Array(values)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Hand-written JSON fixture, not node output or consensus test vectors.
    fn state() -> Value {
        json!({"transition_frontier": {"best_chain": [
            {"block": "first"}, {"block": "second"}
        ]}})
    }

    fn apply(filter: &str) -> RpcStateGetResponse {
        StateFilter::parse(filter)?.evaluate(&state())
    }

    #[test]
    fn state_filter_rejects_bad_input_without_unwinding() {
        for filter in [
            "$.transition_frontier.best_chain[0,-1].block",
            "",
            " ",
            "$.transition_frontier.best_chain[",
            "$[0]]",
            "$.transition_frontier.best_chain[-1]",
            "$.transition_frontier.best_chain[99]",
            "$[18446744073709551616]",
            "$[0,1]",
            "$[0,0]",
            "$[0,1.5]",
            "$['a','b']",
            "$[::0]",
            "$[-1:]",
            "$..*",
            "$[?(@.x)]",
            "$@$",
        ] {
            let result = std::panic::catch_unwind(|| apply(filter));
            assert!(result.is_ok(), "unwound for {filter:?}");
            assert!(result.unwrap().is_err(), "accepted {filter:?}");
        }
    }

    #[test]
    fn state_filter_valid_paths_keep_response_shape() {
        assert_eq!(
            apply("$.transition_frontier.best_chain[0].block").unwrap(),
            json!("first")
        );
        assert_eq!(
            apply("$['transition_frontier'].best_chain[*].block").unwrap(),
            json!(["first", "second"])
        );
        assert_eq!(
            apply("$.transition_frontier.best_chain[0:1].block").unwrap(),
            json!("first")
        );
        assert_eq!(apply("$").unwrap(), state());
    }

    #[test]
    fn state_filter_length_is_bounded_in_bytes() {
        let at_limit = format!("$.{}", "x".repeat(MAX_FILTER_BYTES - 2));
        assert!(StateFilter::parse(&at_limit).is_ok());
        assert!(StateFilter::parse(&(at_limit + "x")).is_err());
        assert!(StateFilter::parse(&format!("$['{}']", "é".repeat(512))).is_err());
        assert!(StateFilter::parse(&format!("${}", "[".repeat(10_000))).is_err());
    }

    #[test]
    fn state_filter_contains_library_panics() {
        // Exercise the actual offending library path, bypassing validation to
        // prove the evaluation boundary still contains a library regression.
        let unsafe_path = StateFilter(JsonPath::try_from("$[0,-1]").unwrap());
        assert!(unsafe_path.evaluate(&json!([1, 2])).is_err());
        assert!(guarded::<()>(|| panic!("parser regression")).is_err());
        assert_eq!(
            apply("$.transition_frontier.best_chain[1].block").unwrap(),
            json!("second")
        );
    }
}
