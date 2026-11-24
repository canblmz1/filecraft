import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('..', import.meta.url))
const desktop = join(root, 'apps', 'desktop')
const text = path => readFileSync(path, 'utf8').trim()

function cargo(args) {
  return execFileSync('cargo', args, { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 })
}

const host = execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1]?.trim()
const target = process.env.TAURI_ENV_TARGET_TRIPLE || host
if (!target) throw new Error('Could not determine the Rust build target')

const { packages } = JSON.parse(cargo(['metadata', '--locked', '--format-version', '1', '--filter-platform', target]))
const dependencyTree = cargo([
  'tree', '--locked', '-p', 'filecraft-desktop', '--target', target,
  '--edges', 'normal', '--prefix', 'none', '--format', '{p}',
])
const runtime = new Set(dependencyTree.split(/\r?\n/).map(line => line.trim().split(' ').slice(0, 2).join(' ')))

const listed = []
for (const pkg of packages) {
  if (!pkg.source || !runtime.has(pkg.name + ' v' + pkg.version)) continue
  listed.push({
    name: pkg.name, version: pkg.version, license: pkg.license, url: pkg.repository,
    directory: dirname(pkg.manifest_path), licenseFile: pkg.license_file,
  })
}

const lock = JSON.parse(text(join(desktop, 'package-lock.json')))
for (const [path, entry] of Object.entries(lock.packages)) {
  if (!path || entry.dev) continue
  const directory = resolve(desktop, path)
  if (!existsSync(directory)) continue
  const pkg = JSON.parse(text(join(directory, 'package.json')))
  listed.push({ name: pkg.name, version: pkg.version, license: pkg.license, directory })
}

const blocks = [
  'FileCraft application license\n\n' + text(join(root, 'LICENSE')),
  'Third-party dependency notices\n\nTarget: ' + target,
]
for (const pkg of listed.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version))) {
  const paths = new Set(readdirSync(pkg.directory, { withFileTypes: true })
    .filter(file => file.isFile() && /^(licen[cs]e|copying|notice|ofl)([._-]|$)/i.test(file.name))
    .map(file => join(pkg.directory, file.name)))
  if (pkg.licenseFile) {
    const path = resolve(pkg.directory, pkg.licenseFile)
    if (existsSync(path)) paths.add(path)
  }
  const header = [pkg.name + ' ' + pkg.version, pkg.license, pkg.url].filter(Boolean).join(' | ')
  blocks.push([header, ...Array.from(paths).sort().map(path => basename(path) + '\n' + text(path))].join('\n\n'))
}
writeFileSync(join(desktop, 'dist', 'third-party-notices.txt'), blocks.join('\n\n' + '-'.repeat(72) + '\n\n') + '\n')
console.log('Wrote third-party notices for ' + listed.length + ' packages.')
