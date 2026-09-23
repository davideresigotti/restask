import esbuild from "esbuild";
import builtins from "builtin-modules";

const context = await esbuild.context({
	entryPoints: ["src/main.ts"],
	bundle: true,
	external: ["obsidian", ...builtins],
	format: "cjs",
	target: "es2022",
	minify: true,
	outfile: "main.js",
	logLevel: "info",
});

await context.rebuild();
context.dispose();
