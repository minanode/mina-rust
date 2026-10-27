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

use juniper::{
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
