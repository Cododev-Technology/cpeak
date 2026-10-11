use serde::Serialize;

use crate::types::{Middleware, Next};
use crate::{CpeakRequest, CpeakResponse};

pub fn swagger(spec: &impl Serialize, prefix: Option<&str>) -> impl Middleware {
    let prefix = prefix.unwrap_or("/api-docs").to_string();
    let spec = serde_json::to_value(spec)
        .unwrap_or_else(|error| panic!("swagger: the spec can't be turned into JSON: {error}"));

    let initializer_js = format!(
        r#"window.onload = function() {{
  SwaggerUIBundle({{
    url: "{prefix}/spec.json",
    dom_id: '#swagger-ui',
    presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
    layout: "StandaloneLayout"
  }});
}};"#
    );

    async move |req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next| {
        if req.url == prefix || req.url == format!("{prefix}/") {
            let location = format!("{prefix}/index.html");
            res.write_head(302, &[("Location", location.as_str())]);
            return res.end("");
        }
        if req.url == format!("{prefix}/spec.json") {
            return res.json(&spec);
        }
        if req.url == format!("{prefix}/swagger-initializer.js") {
            res.set_header("Content-Type", "application/javascript");
            return res.end(initializer_js.clone());
        }
        next()
    }
}
