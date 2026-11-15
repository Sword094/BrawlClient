// Fails if the app's version is written differently in different places, or doesn't match a release tag.
// Usage: node scripts/check-versions.mjs [v0.2.7]    (the tag is optional; the release build passes it)
import { readFileSync } from "node:fs";

const read = (f) => readFileSync(new URL(`../${f}`, import.meta.url), "utf8");
const found = {
  "package.json": JSON.parse(read("package.json")).version,
  "src-tauri/tauri.conf.json": JSON.parse(read("src-tauri/tauri.conf.json")).version,
  "src-tauri/Cargo.toml": (read("src-tauri/Cargo.toml").match(/^version\s*=\s*"([^"]+)"/m) || [])[1],
  "src-tauri/Cargo.lock": (read("src-tauri/Cargo.lock").match(/name = "brawl-stars-hub"\r?\nversion = "([^"]+)"/) || [])[1],
};
const tag = process.argv[2];
if (tag) found[`tag ${tag}`] = tag.replace(/^v/, "");

const problems = [];
const want = found["package.json"];
for (const [file, v] of Object.entries(found)) if (v !== want) problems.push(`${file} says ${v ?? "(not found)"}, expected ${want}`);

const notes = read("RELEASE_NOTES.md");
if (!new RegExp(`What's new in ${want.replace(/\./g, "\\.")}\\b`).test(notes)) problems.push(`RELEASE_NOTES.md has no "What's new in ${want}" heading`);

if (tag) {
  const key = JSON.parse(read("src-tauri/tauri.conf.json")).plugins?.updater?.pubkey;
  if (!key) problems.push("src-tauri/tauri.conf.json has an empty updater pubkey, so installed apps could not verify this update");
}

if (problems.length) {
  console.error("Version check failed:\n - " + problems.join("\n - "));
  process.exit(1);
}
console.log(`Versions agree: ${want}${tag ? " (matches tag)" : ""}`);
