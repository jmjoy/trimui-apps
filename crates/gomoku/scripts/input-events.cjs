// Development-only Linux evdev events for the known TRIMUI Player1 controller.
// This writes test files; it neither opens input devices nor contacts the handheld.
const fs = require('node:fs');
const path = require('node:path');
const dir = process.argv[2];
if (!dir) throw new Error('Usage: node input-events.cjs OUTPUT_DIRECTORY');
fs.mkdirSync(dir, { recursive: true });
function event(type, code, value) {
  const bytes = Buffer.alloc(24); // aarch64 struct input_event
  bytes.writeUInt16LE(type, 16);
  bytes.writeUInt16LE(code, 18);
  bytes.writeInt32LE(value, 20);
  return bytes;
}
function write(name, ...events) {
  fs.writeFileSync(path.join(dir, name + '.bin'), Buffer.concat(events));
}
for (const [name, code] of Object.entries({ a: 305, b: 304, x: 308, y: 307, l: 310, r: 311, start: 315 })) {
  write(name, event(1, code, 1), event(0, 0, 0), event(1, code, 0), event(0, 0, 0));
}
write('up', event(3, 17, -1), event(0, 0, 0));
write('down', event(3, 17, 1), event(0, 0, 0));
write('right', event(3, 16, 1), event(0, 0, 0));
write('left', event(3, 16, -1), event(0, 0, 0));
write('neutral', event(3, 16, 0), event(3, 17, 0), event(3, 0, 0), event(3, 1, 0), event(0, 0, 0));
write('exit-press', event(1, 314, 1), event(1, 315, 1), event(0, 0, 0));
write('exit-release', event(1, 314, 0), event(1, 315, 0), event(0, 0, 0));
