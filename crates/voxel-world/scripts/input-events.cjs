// Development-only Linux evdev events for the TRIMUI Player1 controller.
// This writes test files; it neither opens input devices nor contacts the handheld.
const fs = require('node:fs');
const path = require('node:path');
const dir = process.argv[2];
if (!dir) throw new Error('Usage: node input-events.cjs OUTPUT_DIRECTORY');
fs.rmSync(dir, { recursive: true, force: true });
fs.mkdirSync(dir, { recursive: true });
function event(type, code, value) {
  const bytes = Buffer.alloc(24); // aarch64 struct input_event
  bytes.writeUInt16LE(type, 16);
  bytes.writeUInt16LE(code, 18);
  bytes.writeInt32LE(value, 20);
  return bytes;
}
function write(name, ...events) {
  fs.writeFileSync(path.join(dir, name + '.bin'), Buffer.concat([...events, event(0, 0, 0)]));
}
const buttons = { b: 304, a: 305, y: 307, x: 308, l: 310, r: 311, select: 314, start: 315, menu: 316 };
for (const [name, code] of Object.entries(buttons)) {
  write(name + '-press', event(1, code, 1));
  write(name + '-release', event(1, code, 0));
}
// Left stick ABS_X/Y, right stick ABS_RX/RY; L2/R2 are ABS_Z/ABS_RZ (0..255).
const axes = {
  'move-forward': [1, -32767], 'move-back': [1, 32767],
  'move-left': [0, -32767], 'move-right': [0, 32767],
  'look-up': [4, -22000], 'look-down': [4, 22000],
  'look-left': [3, -22000], 'look-right': [3, 22000],
  place: [2, 255], dig: [5, 255],
};
for (const [name, [code, value]] of Object.entries(axes)) write(name, event(3, code, value));
write('up', event(3, 17, -1));
write('down', event(3, 17, 1));
write('left', event(3, 16, -1));
write('right', event(3, 16, 1));
write('neutral', ...[0, 1, 2, 3, 4, 5, 16, 17].map((c) => event(3, c, 0)),
  ...Object.values(buttons).map((c) => event(1, c, 0)));
write('exit-press', event(1, 314, 1), event(1, 315, 1));
write('exit-release', event(1, 314, 0), event(1, 315, 0));
