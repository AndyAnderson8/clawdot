// The portal shows text written by agents, tools and the network, so it must never turn that text into HTML or code.
// These ESLint rules are the source of truth for that. eslint.config.mjs applies them to the page sources, and
// test/unit/rules.test.ts feeds them known-bad snippets so a rule that quietly stops working fails the unit tests.

const htmlSinks = ["innerHTML", "outerHTML", "insertAdjacentHTML", "createContextualFragment"];

export const safetyRules = {
  "no-eval": "error",
  "no-implied-eval": "error",
  "no-new-func": "error",
  "no-script-url": "error",
  "no-restricted-properties": [
    "error",
    ...htmlSinks.map((property) => ({
      property,
      message: `${property} turns text into HTML; build elements or use JSX text instead.`,
    })),
    { object: "document", property: "write", message: "document.write injects HTML." },
    { object: "document", property: "writeln", message: "document.writeln injects HTML." },
  ],
  "no-restricted-globals": ["error", { name: "DOMParser", message: "DOMParser parses untrusted text as HTML." }],
  "no-restricted-syntax": [
    "error",
    {
      selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']",
      message: "dangerouslySetInnerHTML injects HTML; render text through JSX instead.",
    },
    {
      selector: "Property[key.name='dangerouslySetInnerHTML'], Property[key.value='dangerouslySetInnerHTML']",
      message: "dangerouslySetInnerHTML injects HTML; render text through JSX instead.",
    },
    { selector: "ImportExpression", message: "No dynamic import; the page is one bundle." },
  ],
};
