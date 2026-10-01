import { copyFileSync, mkdirSync } from "node:fs";
import esbuild from "esbuild";
import builtins from "builtin-modules";

const context = await esbuild.context({
	entryPoints: ["src/main.ts"],
	bundle: true,
	// CodeMirror is Obsidian's own copy at run time: bundling a second one would break it.
	external: ["obsidian", "@codemirror/state", "@codemirror/view", ...builtins],
	format: "cjs",
	target: "es2022",
	minify: true,
	outfile: "main.js",
	logLevel: "info",
});

await context.rebuild();
context.dispose();

// `restask setup` installs the plugin from a copy compiled into the binary (§13.2 step 1):
// refresh that copy on every build so the committed bundle is the one just built.
const embedded = "../../crates/restask/assets/obsidian";
mkdirSync(embedded, { recursive: true });
for (const file of ["main.js", "manifest.json", "styles.css"]) {
	copyFileSync(file, `${embedded}/${file}`);
}
