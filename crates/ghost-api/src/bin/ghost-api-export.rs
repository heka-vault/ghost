//! Write the OpenAPI document: `cargo run -p ghost-api --bin ghost-api-export >
//! packages/api/openapi.json`. The committed file is what `packages/client-ts` is
//! generated from; a test fails when it is stale.

fn main() {
    let doc = ghost_api::openapi::document();
    let text = serde_json::to_string_pretty(&doc).expect("the document serializes");
    println!("{text}");
}
