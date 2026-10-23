import { expect, test } from "bun:test";
import { connectBody, emptyForm, endpointProblem, formFromCatalog, formProblem, type CatalogEntry } from "./aiProvider";

const catalog: CatalogEntry[] = [
  { name: "gemini", adapter: "gemini", endpoint: "https://generativelanguage.googleapis.com/v1beta", env: ["GEMINI_API_KEY"], default_model: "gemini-2.5-flash-image", capabilities: ["generate"] },
];

test("endpoints must be https, or http on localhost, without credentials or fragments", () => {
  expect(endpointProblem("https://api.example.com/v1")).toBeNull();
  expect(endpointProblem("http://127.0.0.1:8080/v1")).toBeNull();
  expect(endpointProblem("http://localhost/v1")).toBeNull();
  expect(endpointProblem("http://example.com/v1")).not.toBeNull();
  expect(endpointProblem("https://user:pw@example.com/v1")).not.toBeNull();
  expect(endpointProblem("https://example.com/v1#frag")).not.toBeNull();
  expect(endpointProblem("not a url")).not.toBeNull();
});

test("a catalog provider needs only a key variable name", () => {
  const form = formFromCatalog(catalog[0]!);
  expect(formProblem(form, catalog)).toBeNull();
  expect(connectBody(form).credential_env).toBe("GEMINI_API_KEY");
  expect(connectBody(form).credential_file).toBeUndefined();
});

test("a pasted key is refused rather than sent", () => {
  const form = { ...formFromCatalog(catalog[0]!), credential: "AIzaSyD-example-key-1234567890" };
  expect(formProblem(form, catalog)).toContain("not a variable name");
});

test("a custom provider needs an endpoint and a credential", () => {
  const form = { ...emptyForm(), name: "studio" };
  expect(formProblem(form, catalog)).toContain("endpoint");
  expect(formProblem({ ...form, endpoint: "https://host/v1" }, catalog)).toContain("environment variable");
  expect(formProblem({ ...form, endpoint: "https://host/v1", credential: "STUDIO_KEY" }, catalog)).toBeNull();
  expect(formProblem({ ...form, name: "my provider", endpoint: "https://host/v1", credential: "K" }, catalog)).toContain("name");
});

test("a key file is accepted as a path and sent as credential_file", () => {
  const form = { ...emptyForm(), name: "studio", endpoint: "https://host/v1", credentialKind: "file" as const, credential: "C:/Users/me/key.txt" };
  expect(formProblem(form, catalog)).toBeNull();
  const body = connectBody(form);
  expect(body.credential_file).toBe("C:/Users/me/key.txt");
  expect(body.credential_env).toBeUndefined();
});

test("a model prefix must match the provider name before submitting (#339)", () => {
  const form = { ...formFromCatalog(catalog[0]!), defaultModel: "openai/model-x" };
  expect(formProblem(form, catalog)).toContain('must match the provider name "gemini"');
  expect(formProblem({ ...form, defaultModel: "gemini/model-x" }, catalog)).toBeNull();
  expect(formProblem({ ...form, defaultModel: "model-x" }, catalog)).toBeNull();
});
