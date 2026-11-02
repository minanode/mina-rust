//! OCaml's request handling around Juniper's execution: the variables a
//! request leaves out, and the shape of error responses.
//!
//! - ocaml-graphql-server fails a request whose executed arguments use a
//!   variable the request does not give and that has no default, nullable
//!   or not, with `Missing variable` (Juniper treats an omitted nullable
//!   variable as null).
//! - OCaml's errors have no `locations`.
//! - An argument error replaces the whole response: one error, no `path`,
//!   and `data: null`.
//!
//! OCaml: <https://github.com/andreas/ocaml-graphql-server/blob/0.13.0/graphql/src/graphql_schema.ml#L266-L289>
use std::collections::{HashMap, HashSet};

use crate::graphql::api::{
    http::GraphQLRequest,
    parser::parse_document_source,
    validation::{visit_all_rules, ValidatorContext},
    Definition, InputValue, Operation, Selection,
};
use serde_json::{json, Value};

use super::graphql::Schema;
use crate::graphql::ocaml_errors::ARGUMENT_ERROR;

/// OCaml's response for a request that uses a variable it does not give, or
/// `None`. Documents Juniper refuses are left to Juniper, which reports
/// their errors first, as does a request whose operation can't be chosen.
pub(super) fn missing_variable(request: &GraphQLRequest, schema: &Schema) -> Option<Value> {
    let document = parse_document_source(&request.query, &schema.schema).ok()?;
    let mut validator = ValidatorContext::new(&schema.schema, &document);
    visit_all_rules(&mut validator, &document);
    if !validator.into_errors().is_empty() {
        return None;
    }
    let mut operations = document.iter().filter_map(|definition| match definition {
        Definition::Operation(operation) => Some(&operation.item),
        Definition::Fragment(_) => None,
    });
    let operation = match request.operation_name.as_deref() {
        Some(name) => {
            operations.find(|operation| operation.name.as_ref().map(|n| n.item) == Some(name))?
        }
        None => {
            let operation = operations.next()?;
            if operations.next().is_some() {
                return None;
            }
            operation
        }
    };
    let fragments: HashMap<&str, &[Selection<'_, _>]> = document
        .iter()
        .filter_map(|definition| match definition {
            Definition::Fragment(fragment) => Some((
                fragment.item.name.item,
                fragment.item.selection_set.as_slice(),
            )),
            Definition::Operation(_) => None,
        })
        .collect();
    let given: HashSet<&str> = request
        .variables
        .as_ref()
        .and_then(InputValue::to_object_value)
        .map(|variables| variables.into_keys().collect())
        .unwrap_or_default();
    let defaults = defaulted_variables(operation);
    let mut used = Vec::new();
    collect_variables(
        &operation.selection_set,
        &fragments,
        &mut Vec::new(),
        &mut used,
    );
    used.into_iter()
        .find(|name| !given.contains(name) && !defaults.contains(name))
        .map(|name| {
            json!({
                "errors": [{ "message": format!("Missing variable `{name}`") }],
                "data": null,
            })
        })
}

fn defaulted_variables<'a, S>(operation: &Operation<'a, S>) -> HashSet<&'a str> {
    operation
        .variable_definitions
        .iter()
        .flat_map(|definitions| &definitions.item.items)
        .filter(|(_, definition)| definition.default_value.is_some())
        .map(|(name, _)| name.item)
        .collect()
}

/// The variables of a list of directives' arguments. A macro, as Juniper
/// does not export the directive's type.
macro_rules! directive_variables {
    ($directives:expr, $used:expr) => {
        for directive in $directives.iter().flatten() {
            for (_, value) in directive.item.arguments.iter().flat_map(|a| &a.item.items) {
                input_variables(&value.item, $used);
            }
        }
    };
}

/// The variables of the arguments of fields and directives, in document
/// order, through fragments. Validation has ruled out fragment cycles; the
/// active list stops one anyway.
fn collect_variables<'a, S>(
    selections: &'a [Selection<'a, S>],
    fragments: &HashMap<&'a str, &'a [Selection<'a, S>]>,
    active: &mut Vec<&'a str>,
    used: &mut Vec<&'a str>,
) {
    for selection in selections {
        match selection {
            Selection::Field(field) => {
                let field = &field.item;
                directive_variables!(field.directives, used);
                for (_, value) in field.arguments.iter().flat_map(|a| &a.item.items) {
                    input_variables(&value.item, used);
                }
                if let Some(children) = &field.selection_set {
                    collect_variables(children, fragments, active, used);
                }
            }
            Selection::InlineFragment(fragment) => {
                directive_variables!(fragment.item.directives, used);
                collect_variables(&fragment.item.selection_set, fragments, active, used);
            }
            Selection::FragmentSpread(spread) => {
                directive_variables!(spread.item.directives, used);
                let name = spread.item.name.item;
                if let Some(children) = fragments.get(name).filter(|_| !active.contains(&name)) {
                    active.push(name);
                    collect_variables(children, fragments, active, used);
                    active.pop();
                }
            }
        }
    }
}

fn input_variables<'a, S>(value: &'a InputValue<S>, used: &mut Vec<&'a str>) {
    match value {
        InputValue::Variable(name) => used.push(name),
        InputValue::List(items) => items.iter().for_each(|v| input_variables(&v.item, used)),
        InputValue::Object(fields) => fields
            .iter()
            .for_each(|(_, v)| input_variables(&v.item, used)),
        InputValue::Null | InputValue::Scalar(_) | InputValue::Enum(_) => {}
    }
}

/// A Juniper response in OCaml's shape: no `locations`, and an argument
/// error as the whole response.
///
/// OCaml: <https://github.com/andreas/ocaml-graphql-server/blob/0.13.0/graphql/src/graphql_schema.ml#L1364-L1502>
pub(super) fn post_process(mut response: Value) -> Value {
    let Some(errors) = response.get_mut("errors").and_then(Value::as_array_mut) else {
        return response;
    };
    if let Some(error) = errors
        .iter()
        .find(|error| error.pointer(&format!("/extensions/{ARGUMENT_ERROR}")) == Some(&json!(true)))
    {
        return json!({ "errors": [{ "message": error["message"] }], "data": null });
    }
    for error in errors {
        if let Some(error) = error.as_object_mut() {
            error.remove("locations");
        }
    }
    response
}

// A parsing-only schema leaves opaque scalar contents untyped. Juniper's
// schema-aware parser otherwise refuses object literals before scalar coercion.
// Execution and validation always use the real schema, never this one.
struct JsonParsingQuery;
#[juniper::graphql_object(scalar = crate::graphql::scalars::MinaScalarValue)]
impl JsonParsingQuery {
    fn integer() -> crate::graphql::scalars::CommandNonceScalar {
        crate::graphql::scalars::CommandNonceScalar(0)
    }
    fn float() -> f64 {
        0.0
    }
}

pub(super) fn parse_archive_document<'a>(
    query: &'a str,
    schema: &Schema,
) -> juniper::parser::UnlocatedParseResult<
    Vec<Definition<'a, crate::graphql::scalars::MinaScalarValue>>,
> {
    parse_document_source(query, &schema.schema).or_else(|error| {
        if !query.contains("archiveVersionedPrecomputedBlock") {
            return Err(error);
        }
        static PARSING: std::sync::OnceLock<
            juniper::SchemaType<crate::graphql::scalars::MinaScalarValue>,
        > = std::sync::OnceLock::new();
        let parsing = PARSING.get_or_init(|| {
            juniper::SchemaType::new::<
                JsonParsingQuery,
                juniper::EmptyMutation<()>,
                juniper::EmptySubscription<()>,
            >(&(), &(), &())
        });
        parse_document_source(query, parsing)
    })
}

/// Adapt OCaml's opaque JSON input scalar to Juniper, which otherwise rejects
/// objects/lists even when the scalar's FromInputValue accepts them. Admission
/// and public policy run on the original request before this adapter. Only the
/// versioned archive scalar is affected; null and wrong variable types retain
/// Juniper's normal validation. The archive is unconfigured, so the resolver
/// returns the upstream configuration error and never forwards this encoding.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml#L3011-L3023>
pub(super) fn archive_json_input(
    request: &GraphQLRequest,
    schema: &Schema,
) -> Option<GraphQLRequest> {
    use crate::graphql::api::parser::Spanning;
    fn opaque(value: &InputValue) -> bool {
        matches!(value, InputValue::Object(_) | InputValue::List(_))
            && value.referenced_variables().is_empty()
    }
    fn replacement(value: &Spanning<InputValue>, edits: &mut Vec<(usize, usize, String)>) {
        if opaque(&value.item) {
            let json = serde_json::to_string(&value.item).expect("InputValue serializes");
            edits.push((
                value.span.start.index(),
                value.span.end.index(),
                serde_json::to_string(&json).expect("String serializes"),
            ));
        }
    }
    fn walk(
        selections: &[Selection<'_, crate::graphql::scalars::MinaScalarValue>],
        edits: &mut Vec<(usize, usize, String)>,
    ) {
        for selection in selections {
            match selection {
                Selection::Field(field) => {
                    if field.item.name.item == "archiveVersionedPrecomputedBlock" {
                        for (name, value) in field.item.arguments.iter().flat_map(|a| &a.item.items)
                        {
                            if name.item == "block" {
                                replacement(value, edits);
                            }
                        }
                    }
                    if let Some(children) = &field.item.selection_set {
                        walk(children, edits);
                    }
                }
                Selection::InlineFragment(fragment) => walk(&fragment.item.selection_set, edits),
                Selection::FragmentSpread(_) => {}
            }
        }
    }
    if !request.query.contains("archiveVersionedPrecomputedBlock") {
        return None;
    }
    let doc = parse_archive_document(&request.query, schema).ok()?;
    let selected =
        juniper::executor::get_operation(&doc, request.operation_name.as_deref()).ok()?;
    let names: HashSet<_> = selected
        .item
        .variable_definitions
        .iter()
        .flat_map(|d| &d.item.items)
        .filter(|(_, d)| {
            d.var_type.item.to_string().trim_end_matches('!') == "VersionedPrecomputedBlock"
        })
        .map(|(name, _)| name.item)
        .collect();
    let mut variables = request.variables.clone();
    let mut changed = false;
    if let Some(InputValue::Object(entries)) = &mut variables {
        for (name, value) in entries {
            if names.contains(name.item.as_str()) && opaque(&value.item) {
                value.item = InputValue::scalar(
                    serde_json::to_string(&value.item).expect("InputValue serializes"),
                );
                changed = true;
            }
        }
    }
    let mut edits = vec![];
    for definition in &doc {
        match definition {
            Definition::Operation(op) => {
                walk(&op.item.selection_set, &mut edits);
                for (_, d) in op
                    .item
                    .variable_definitions
                    .iter()
                    .flat_map(|d| &d.item.items)
                {
                    if d.var_type.item.to_string().trim_end_matches('!')
                        == "VersionedPrecomputedBlock"
                    {
                        if let Some(value) = &d.default_value {
                            replacement(value, &mut edits);
                        }
                    }
                }
            }
            Definition::Fragment(f) => walk(&f.item.selection_set, &mut edits),
        }
    }
    if !changed && edits.is_empty() {
        return None;
    }
    // Juniper positions count Unicode scalar values, not UTF-8 bytes.
    let offsets: Vec<_> = request
        .query
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(request.query.len()))
        .collect();
    let mut query = request.query.clone();
    edits.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    for (start, end, replacement) in edits {
        query.replace_range(*offsets.get(start)?..*offsets.get(end)?, &replacement);
    }
    Some(GraphQLRequest::new(
        query,
        request.operation_name.clone(),
        variables,
    ))
}
