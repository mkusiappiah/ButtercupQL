// The bundled extension, as the package holds it, loads and gives VS Code its two functions.

"use strict";

const assert = require("assert");
const Module = require("module");
const path = require("path");

// A test stand-in for VS Code: every name is there and does nothing.
const nothing = new Proxy(function () {}, { get: () => nothing, apply: () => nothing });
const load = Module._load;
Module._load = function (request, parent, isMain) {
  return request === "vscode" ? nothing : load.call(this, request, parent, isMain);
};

const extension = require(path.join(__dirname, "..", "dist", "extension.js"));
assert.strictEqual(typeof extension.activate, "function");
assert.strictEqual(typeof extension.deactivate, "function");
console.log("bundle: it loads");
