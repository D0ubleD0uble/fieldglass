// ESLint flat config for the Fieldglass VS Code extension.
// Lints TypeScript sources only — `out/`, `bin/`, `node_modules/`, and the
// downloaded VS Code test instance are excluded.

import tsParser from "@typescript-eslint/parser";
import tsPlugin from "@typescript-eslint/eslint-plugin";
import security from "eslint-plugin-security";

export default [
  {
    ignores: [
      "out/**",
      "bin/**",
      "node_modules/**",
      ".vscode-test/**",
      "media/**",
    ],
  },
  {
    files: ["src/**/*.ts"],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaVersion: 2020,
        sourceType: "module",
        project: "./tsconfig.json",
      },
    },
    plugins: {
      "@typescript-eslint": tsPlugin,
      security,
    },
    rules: {
      // typescript-eslint recommended baseline.
      ...tsPlugin.configs["recommended"].rules,

      // Security plugin recommended baseline — pattern-matching SAST that
      // catches eval/exec/non-literal-fs/unsafe-regex categories. Pairs with
      // the workflow Semgrep scan (which runs more rules less often).
      ...security.configs.recommended.rules,

      // Tactical relaxations:
      // - `_`-prefixed unused vars are explicit "I know" markers (e.g. unused
      //   parameters of trait-shaped callbacks).
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },
  {
    // Test code reads bundled fixtures via fs.readFileSync and indexes into
    // typed Buffer/Uint8Array with constant offsets. Those legitimately
    // trigger detect-non-literal-fs-filename and detect-object-injection in
    // patterns that are perfectly safe in a test harness — disable the
    // security plugin here. Production code under src/**/*.ts (excluding
    // test/) keeps the full ruleset.
    files: ["src/test/**/*.ts"],
    rules: {
      "security/detect-non-literal-fs-filename": "off",
      "security/detect-object-injection": "off",
    },
  },
];
