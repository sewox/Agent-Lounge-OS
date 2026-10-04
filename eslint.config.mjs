import { defineConfig, globalIgnores } from "eslint/config";
import nextVitals from "eslint-config-next/core-web-vitals";
import nextTs from "eslint-config-next/typescript";

const eslintConfig = defineConfig([
  ...nextVitals,
  ...nextTs,
  {
    files: ["src/components/**/*.{ts,tsx}", "src/app/**/*.{ts,tsx}"],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector: "CallExpression[callee.name='paletteShortcutLabel'][arguments.length=0]",
          message:
            "Pass explicit platform from usePlatform() to paletteShortcutLabel(platform) for SSR-stable renders.",
        },
        {
          selector: "CallExpression[callee.name='detectPlatform']",
          message:
            "Use usePlatform() instead of detectPlatform() in UI code; detectPlatform() belongs in hooks/effects only.",
        },
        {
          // Date.now() / Math.random() inside JSX expressions (render output).
          selector:
            "JSXExpressionContainer CallExpression[callee.object.name='Date'][callee.property.name='now']",
          message:
            "Date.now() in JSX can cause hydration mismatch; defer to effects or SSR-stable placeholders.",
        },
        {
          selector:
            "JSXExpressionContainer CallExpression[callee.object.name='Math'][callee.property.name='random']",
          message:
            "Math.random() in JSX can cause hydration mismatch; defer to effects or SSR-stable placeholders.",
        },
        {
          // useState(() => Date.now()) / useState(Date.now()) initializers diverge SSR vs client.
          selector:
            "CallExpression[callee.name='useState'] CallExpression[callee.object.name='Date'][callee.property.name='now']",
          message:
            "Date.now() in useState initializers can cause hydration mismatch; use a stable placeholder then update in an effect.",
        },
        {
          selector:
            "CallExpression[callee.name='useState'] CallExpression[callee.object.name='Math'][callee.property.name='random']",
          message:
            "Math.random() in useState initializers can cause hydration mismatch; use a stable placeholder then update in an effect.",
        },
        {
          selector:
            "JSXExpressionContainer MemberExpression[object.name='navigator']",
          message:
            "Direct navigator access in JSX can cause hydration mismatch; use usePlatform() or effects.",
        },
        {
          selector:
            "CallExpression[callee.name='useState'] MemberExpression[object.name='navigator']",
          message:
            "navigator in useState initializers can cause hydration mismatch; use usePlatform() or effects.",
        },
      ],
    },
  },
  // Override default ignores of eslint-config-next.
  globalIgnores([
    // Default ignores of eslint-config-next:
    ".next/**",
    "out/**",
    "build/**",
    "next-env.d.ts",
    "e2e/**",
    "playwright-report/**",
    "test-results/**",
    "scripts/qa/**",
    "src-tauri/target/**",
    // Standalone Node harness (own package; not Next/app code)
    "tools/mcp-probe/**",
  ]),
]);

export default eslintConfig;
