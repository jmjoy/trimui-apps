#!/usr/bin/env node
// Host-only integration tests: all Cargo and ADB calls use local substitutes.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const repository = path.resolve(__dirname, '..');
fs.mkdirSync(path.join(repository, 'artifacts'), { recursive: true });
const temporary = fs.mkdtempSync(path.join(repository, 'artifacts/workflow-tests-'));
const workspace = path.join(temporary, 'workspace with spaces');
const bin = path.join(temporary, 'bin');
const device = path.join(temporary, 'device');
const deviceTmp = path.join(temporary, 'device-tmp');
const callsFile = path.join(temporary, 'calls.jsonl');
const projects = [
  ['chinese-chess', 'ChineseChess', 'chinese_chess'],
  ['gomoku', 'Gomoku', 'gomoku'],
  ['voxel-world', 'VoxelWorld', 'voxel_world'],
];
for (const directory of [workspace, bin, device, deviceTmp]) fs.mkdirSync(directory, { recursive: true });
fs.writeFileSync(path.join(bin, 'package.json'), '{"type":"commonjs"}');
for (const file of ['justfile', 'Cargo.toml', 'scripts']) {
  fs.cpSync(path.join(repository, file), path.join(workspace, file), { recursive: true });
}
for (const [slug] of projects) {
  fs.cpSync(path.join(repository, 'crates', slug), path.join(workspace, 'crates', slug), {
    recursive: true,
    filter: (file) => !['target', 'artifacts', 'src'].includes(path.basename(file)),
  });
}
for (const file of ['pkgconfig/alsa.pc', 'device-libs/libasound.so.2']) {
  const destination = path.join(workspace, 'toolchain', file);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, 'fixture');
}

const env = {
  ...process.env,
  PATH: `${bin}:${process.env.PATH}`,
  TEST_WORKSPACE: workspace,
  TEST_DEVICE: device,
  TEST_DEVICE_TMP: deviceTmp,
  TEST_CALLS: callsFile,
};
const stubHeader = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const args = process.argv.slice(2);
fs.appendFileSync(process.env.TEST_CALLS, JSON.stringify({tool: path.basename(process.argv[1]), args}) + '\\n');
`;
function stub(name, code) {
  fs.writeFileSync(path.join(bin, name), stubHeader + code, { mode: 0o755 });
}
stub('cargo', `
if (args[0] === 'zigbuild') {
  const names = args.includes('--workspace')
    ? ['trimui-chinese-chess', 'trimui-gomoku', 'trimui-voxel-world']
    : [args[args.indexOf('-p') + 1]];
  const profile = args.includes('--release') ? 'release' : 'debug';
  const output = path.join(process.env.TEST_WORKSPACE, 'target/aarch64-unknown-linux-gnu', profile);
  fs.mkdirSync(output, {recursive: true});
  for (const name of names) fs.writeFileSync(path.join(output, name), name + ' ' + profile, {mode: 0o755});
}
`);
stub('adb', `
const map = (value) => value.replaceAll('/mnt/SDCARD', process.env.TEST_DEVICE).replaceAll('/tmp/', process.env.TEST_DEVICE_TMP + '/');
if (args[0] === 'shell') {
  const result = spawnSync('/bin/sh', ['-c', map(args.slice(1).join(' '))], {env: process.env, encoding: 'utf8'});
  let output = (result.stdout || '').replaceAll(process.env.TEST_DEVICE, '/mnt/SDCARD');
  if (process.env.TEST_OMIT_RESULT) output = output.replace('Deploy result: PASS\\n', '');
  // Include CRLF output and emulate adbd losing the remote exit status.
  process.stdout.write(output.replaceAll('\\n', '\\r\\n'));
  process.stderr.write(result.stderr || '');
  process.exit(0);
}
if (args[0] === 'push') {
  if (process.env.TEST_PUSH_FAIL) process.exit(1);
  const destination = map(args[2]);
  fs.cpSync(args[1], destination, {recursive: true});
  if (process.env.TEST_CORRUPT_PACKAGE && destination.endsWith('package.tar.gz')) fs.appendFileSync(destination, 'corrupt');
}
if (args[0] === 'pull') fs.writeFileSync(args[2], 'capture');
`);
stub('sh', `
if (args[0].endsWith('/test.sh')) {
  process.stdout.write('First frame: present completed\\n');
  const label = args[0].includes('ChineseChess') ? 'Chinese Chess' : args[0].includes('Gomoku') ? 'Gomoku' : 'Voxel World';
  process.stdout.write(label + ' exit status: 0\\nCPU online after exit: 0-1\\n');
} else {
  const result = spawnSync('/bin/sh', args, {env: process.env, stdio: 'inherit'});
  process.exit(result.status ?? 1);
}
`);
stub('cp', `
if (process.env.TEST_COPY_FAIL && args.at(-1).endsWith('/Ports/Gomoku/trimui-gomoku')) process.exit(1);
const result = spawnSync('/bin/cp', args, {stdio: 'inherit'});
if (process.env.TEST_CORRUPT_INSTALLED && args.at(-1).endsWith('/Ports/Gomoku/trimui-gomoku')) fs.appendFileSync(args.at(-1), 'corrupt');
process.exit(result.status ?? 1);
`);

function run(command, args, options = {}) {
  const { failure = false, ...extra } = options;
  const result = spawnSync(command, args, { cwd: workspace, env, encoding: 'utf8', ...extra });
  if (failure) assert.notEqual(result.status, 0, `Expected failure: ${command} ${args.join(' ')}`);
  else assert.equal(result.status, 0, `${command} ${args.join(' ')}\n${result.stdout}\n${result.stderr}`);
  return result;
}
function just(...args) { return run('just', args); }
function calls() {
  return fs.existsSync(callsFile) ? fs.readFileSync(callsFile, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse) : [];
}
function resetCalls() { fs.writeFileSync(callsFile, ''); }
function cargoCalls() { return calls().filter((call) => call.tool === 'cargo'); }
function writeDevice(file, contents = 'keep') {
  const destination = path.join(device, file);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, contents);
}
function readDevice(file) { return fs.readFileSync(path.join(device, file), 'utf8'); }
function assertNoAppStarted() {
  assert(!calls().some((call) => call.tool === 'sh' && call.args[0].endsWith('/test.sh')));
}
function assertNoTemporaryFiles() {
  assert(!fs.readdirSync(device).some((file) => file.startsWith('.trimui-apps-deploy.')));
  assert(!fs.readdirSync(path.join(workspace, 'artifacts')).some((file) => file.startsWith('.deploy-manifest.')));
}

try {
  // Default commands cover the workspace; explicit module commands remain scoped.
  for (const command of ['fmt', 'test', 'clippy']) {
    resetCalls();
    just(command);
    assert.equal(cargoCalls().length, 1);
    assert(cargoCalls()[0].args.includes(command === 'fmt' ? '--all' : '--workspace'));
    resetCalls();
    just(command, 'gomoku');
    assert(cargoCalls()[0].args.includes('trimui-gomoku'));
    assert(!cargoCalls()[0].args.includes('--all'));
  }
  resetCalls();
  just('check');
  assert.deepEqual(cargoCalls().map((call) => call.args[0]), ['fmt', 'test', 'clippy']);
  resetCalls();
  just('build');
  assert.equal(cargoCalls().length, 1);
  assert(cargoCalls()[0].args.includes('--workspace'));
  assert(cargoCalls()[0].args.includes('aarch64-unknown-linux-gnu.2.33'));
  resetCalls();
  just('build', 'gomoku', 'debug');
  assert(!cargoCalls()[0].args.includes('--release'));
  assert(cargoCalls()[0].args.includes('trimui-gomoku'));
  for (const args of [['build', 'missing'], ['package', 'all', 'invalid']]) {
    resetCalls();
    run('just', args, { failure: true });
    assert.equal(cargoCalls().length, 0);
  }

  // The aggregate contains precisely the install files, never stale apps or logs.
  const artifactRoot = path.join(workspace, 'artifacts');
  fs.mkdirSync(path.join(artifactRoot, 'Ports/StaleApp'), { recursive: true });
  fs.writeFileSync(path.join(artifactRoot, 'Ports/StaleApp/stale.log'), 'stale');
  resetCalls();
  just('package');
  assert.equal(cargoCalls().length, 1);
  assert.equal(fs.readdirSync(artifactRoot).filter((name) => name.endsWith('.tar.gz')).length, 1);
  const archive = path.join(artifactRoot, 'trimui-apps-trimui-smart-pro.tar.gz');
  assert.equal(fs.statSync(archive).mode & 0o777, 0o644);
  const entries = run('tar', ['-tzf', archive]).stdout.trim().split('\n');
  assert.equal(entries.length, 18);
  for (const [slug, port] of projects) {
    for (const file of [`trimui-${slug}`, 'launch.sh', 'test.sh', 'config.json', 'icon.png', `.trimui-${slug}`]) {
      assert(entries.includes(`Ports/${port}/${file}`));
    }
    assert(fs.statSync(path.join(artifactRoot, 'Ports', port, 'launch.sh')).mode & 0o100);
  }
  just('package', 'gomoku', 'debug');
  const singleEntries = run('tar', ['-tzf', path.join(artifactRoot, 'gomoku-trimui-smart-pro.tar.gz')]).stdout.trim().split('\n');
  assert.equal(singleEntries.length, 6);
  assert(singleEntries.every((entry) => entry.startsWith('Ports/Gomoku/')));
  assert.equal(fs.readFileSync(path.join(artifactRoot, 'Ports/Gomoku/trimui-gomoku'), 'utf8'), 'trimui-gomoku debug');

  // Run from project directories too, with safely forwarded arbitrary arguments.
  for (const [slug] of projects) {
    resetCalls();
    run('just', ['build', 'debug'], { cwd: path.join(workspace, 'crates', slug) });
    assert(cargoCalls()[0].args.includes(`trimui-${slug}`));
  }
  const controller = "a b's;$(touch SHOULD_NOT_EXIST)";
  const capture = "/tmp/a b's;$(touch SHOULD_NOT_EXIST).png";
  resetCalls();
  just('gomoku::run', '--', '--seconds', '15', '--controller', controller, '--capture', capture);
  const appCall = calls().find((call) => call.tool === 'sh' && call.args[0].endsWith('/test.sh'));
  assert.deepEqual(appCall.args.slice(1), ['--seconds', '15', '--controller', controller, '--capture', capture.replace('/tmp/', `${deviceTmp}/`)]);
  assert(!fs.existsSync(path.join(workspace, 'SHOULD_NOT_EXIST')));

  // A bulk install preflights every marker before overwriting the first app.
  for (const [slug, port] of projects) writeDevice(`Ports/${port}/.trimui-${slug}`, '');
  writeDevice('Ports/ChineseChess/trimui-chinese-chess', 'old binary');
  fs.rmSync(path.join(device, 'Ports/VoxelWorld/.trimui-voxel-world'));
  resetCalls();
  run('just', ['deploy'], { failure: true });
  assert.equal(readDevice('Ports/ChineseChess/trimui-chinese-chess'), 'old binary');
  assertNoAppStarted();
  assertNoTemporaryFiles();

  writeDevice('Ports/VoxelWorld/.trimui-voxel-world', '');
  writeDevice('Ports/VoxelWorld/save/world.dat');
  writeDevice('Ports/VoxelWorld/config/settings.json');
  writeDevice('Ports/VoxelWorld/voxel-world.log');
  writeDevice('Apps/ChineseChess/.trimui-chinese-chess', '');
  resetCalls();
  just('deploy');
  assertNoAppStarted();
  assertNoTemporaryFiles();
  assert.equal(calls().filter((call) => call.tool === 'adb' && call.args[0] === 'push' && call.args[2].endsWith('package.tar.gz')).length, 1);
  for (const file of ['save/world.dat', 'config/settings.json', 'voxel-world.log']) assert.equal(readDevice(`Ports/VoxelWorld/${file}`), 'keep');
  assert(!fs.existsSync(path.join(device, 'Apps/ChineseChess')));
  for (const [slug, port] of projects) assert.equal(readDevice(`Ports/${port}/trimui-${slug}`), `trimui-${slug} release`);
  resetCalls();
  just('gomoku::deploy', 'debug');
  assert.equal(readDevice('Ports/Gomoku/trimui-gomoku'), 'trimui-gomoku debug');
  assertNoAppStarted();

  // Package corruption, install corruption, transport errors and missing result
  // lines must fail even though the fake adbd always returns zero for shell.
  for (const flag of ['TEST_CORRUPT_PACKAGE', 'TEST_CORRUPT_INSTALLED', 'TEST_PUSH_FAIL', 'TEST_OMIT_RESULT']) {
    resetCalls();
    run('just', ['deploy', 'gomoku'], { env: { ...env, [flag]: '1' }, failure: true });
    assertNoAppStarted();
    assertNoTemporaryFiles();
  }
  fs.rmSync(path.join(device, 'Ports/Gomoku'), { recursive: true });
  run('just', ['deploy', 'gomoku'], { env: { ...env, TEST_COPY_FAIL: '1' }, failure: true });
  assert(fs.existsSync(path.join(device, 'Ports/Gomoku/.trimui-gomoku')));
  just('deploy', 'gomoku');
  assert.equal(readDevice('Ports/Gomoku/trimui-gomoku'), 'trimui-gomoku release');
  assertNoTemporaryFiles();
  writeDevice('Apps/ChineseChess/user-file');
  fs.rmSync(path.join(device, 'Ports/ChineseChess'), { recursive: true });
  just('deploy', 'chinese_chess');
  assert.equal(readDevice('Apps/ChineseChess/user-file'), 'keep');

  // Smoke tests remain separate tasks with deployment dependencies.
  for (const [, , id] of projects) {
    const dryRun = just('--dry-run', `${id}::smoke-test`).stderr;
    assert(dryRun.includes('deploy.sh'));
    assert(dryRun.includes('smoke-test.sh'));
  }
  for (const command of ['run', 'input-test', 'bench', 'exit-test']) run('just', ['--show', command], { failure: true });
  resetCalls();
  just('clean', 'chinese_chess');
  assert(cargoCalls()[0].args.includes('trimui-chinese-chess'));
  assert(fs.existsSync(path.join(artifactRoot, 'Ports/Gomoku/trimui-gomoku')));
  assert(!fs.existsSync(archive));
  resetCalls();
  just('clean');
  assert.equal(cargoCalls().length, 1);
  assert(cargoCalls()[0].args.includes('--workspace'));
  console.log('Workflow tests: PASS (host substitutes only)');
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
