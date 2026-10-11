import type { CpeakRequest, CpeakResponse, Next } from "../types";

const swagger = (spec: object, prefix = "/api-docs") => {
  const initializerJs = `window.onload = function() {
  SwaggerUIBundle({
    url: "${prefix}/spec.json",
    dom_id: '#swagger-ui',
    presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
    layout: "StandaloneLayout"
  });
};`;

  return (req: CpeakRequest, res: CpeakResponse, next: Next) => {
    if (req.url === prefix || req.url === `${prefix}/`) {
      res.writeHead(302, { Location: `${prefix}/index.html` });
      res.end();
      return;
    }
    if (req.url === `${prefix}/spec.json`) {
      return res.json(spec);
    }
    if (req.url === `${prefix}/swagger-initializer.js`) {
      res.setHeader("Content-Type", "application/javascript");
      res.end(initializerJs);
      return;
    }
    next();
  };
};

export { swagger };
