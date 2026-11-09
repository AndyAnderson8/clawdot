// ESLint flat config for the portal. Run from portal/ as `npm run lint` (which passes --config lint/eslint.config.mjs).
// It lives in the lint workspace because typescript-eslint needs the TypeScript 6 API installed there (see lint/package.json).
import { fileURLToPath } from "node:url";
import js from "@eslint/js";
import jsxA11y from "eslint-plugin-jsx-a11y";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

import { safetyRules } from "./safety.mjs";

const portal = fileURLToPath(new URL("..", import.meta.url));

export default tseslint.config(
  { ignores: ["node_modules/**", "lint/node_modules/**", "**/*.d.ts"] },

  js.configs.recommended,

  {
    files: ["**/*.{ts,tsx}"],
    extends: [tseslint.configs.recommendedTypeChecked, tseslint.configs.stylisticTypeChecked],
    languageOptions: {
      parserOptions: { project: ["./tsconfig.test.json"], tsconfigRootDir: portal },
    },
    rules: {
      // Types
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-floating-promises": [
        "error",
        { allowForKnownSafeCalls: [{ from: "package", package: "node:test", name: ["test", "describe", "it"] }] },
      ],
      // Preact handlers may be async (onClick={save}); every one reports failure to the person through say()/keep().
      "@typescript-eslint/no-misused-promises": ["error", { checksVoidReturn: { attributes: false } }],
      // `||` on a string means "empty counts as missing", which is the intent in forms and labels.
      "@typescript-eslint/prefer-nullish-coalescing": ["error", { ignorePrimitives: { string: true } }],
      // tsconfig sets noPropertyAccessFromIndexSignature-style access on env and records; brackets are deliberate there.
      "@typescript-eslint/dot-notation": "off",
      "@typescript-eslint/switch-exhaustiveness-check": ["error", { considerDefaultExhaustiveForUnions: false }],
      "@typescript-eslint/consistent-type-imports": [
        "error",
        { prefer: "type-imports", fixStyle: "inline-type-imports" },
      ],
      "@typescript-eslint/consistent-type-exports": "error",
      "@typescript-eslint/explicit-module-boundary-types": "error",
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }],
      "@typescript-eslint/no-non-null-assertion": "error",
      "@typescript-eslint/no-unnecessary-condition": "error",
      // types.ts lists the known values of open-ended strings ("builtin" | "http" | string) as documentation.
      "@typescript-eslint/no-redundant-type-constituents": "off",
      // `interface` for object shapes is the house style (types.ts); allow either so it is not churn.
      "@typescript-eslint/consistent-type-definitions": "off",
      // Dead code
      "no-unused-private-class-members": "error",
      "no-unreachable-loop": "error",
      "no-useless-return": "error",
      // Safety: no HTML injection, no dynamic code (lint/safety.mjs, also exercised by test/unit/rules.test.ts)
      ...safetyRules,
      // Plain readable code
      eqeqeq: ["error", "always"],
      "prefer-const": "error",
      "no-var": "error",
    },
  },

  // The page: Preact components. Hooks rules apply (rules-of-hooks, exhaustive-deps; the React-compiler rules in the
  // plugin's "recommended" set do not apply to Preact and are left out), plus accessibility.
  {
    files: ["src/**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    languageOptions: { globals: globals.browser },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
    },
  },
  {
    files: ["src/**/*.tsx"],
    ...jsxA11y.flatConfigs.recommended,
    rules: {
      ...jsxA11y.flatConfigs.recommended.rules,
      // Preact core spells the attribute `for`, which this rule does not know (it looks for React's htmlFor), so every
      // `<label for=...>` is reported. Labels go through the Field component and the browser smoke test.
      "jsx-a11y/label-has-associated-control": "off",
      // The tab strip is a <nav role="tablist">.
      "jsx-a11y/no-noninteractive-element-to-interactive-role": ["error", { nav: ["tablist"] }],
    },
  },

  // Tests and tooling run in Node (the browser smoke test also touches the page through a script string).
  {
    files: ["test/**/*.ts", "*.mjs", "scripts/**/*.mjs", "lint/*.mjs"],
    languageOptions: { globals: { ...globals.node } },
  },
  {
    files: ["test/**/*.ts"],
    rules: {
      // Test code asserts on values it has just narrowed or knows exist.
      "@typescript-eslint/no-non-null-assertion": "off",
      "@typescript-eslint/no-unnecessary-condition": "off",
      "@typescript-eslint/explicit-module-boundary-types": "off",
    },
  },

  // The safety test loads ESLint dynamically and holds the bad snippets as strings.
  {
    files: ["test/unit/rules.test.ts"],
    rules: { "no-restricted-syntax": "off", "no-script-url": "off" },
  },

  // The fake device answers with whatever JSON the page sent; typing every body would just restate the Rust API.
  {
    files: ["test/browser/mock-api.ts"],
    rules: {
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unsafe-member-access": "off",
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/no-unsafe-return": "off",
      "@typescript-eslint/no-unsafe-call": "off",
    },
  },

  // Plain .mjs tooling is not type-checked.
  { files: ["**/*.mjs"], extends: [tseslint.configs.disableTypeChecked] },
);
