// Short synthesized cues (no audio files). Kept quiet and brief.

let ctx: AudioContext | null = null;

function tone(freq: number, start: number, dur: number, type: OscillatorType = "square", gain = 0.05) {
  ctx ??= new AudioContext();
  const t0 = ctx.currentTime + start;
  const osc = ctx.createOscillator();
  const g = ctx.createGain();
  osc.type = type;
  osc.frequency.setValueAtTime(freq, t0);
  g.gain.setValueAtTime(0, t0);
  g.gain.linearRampToValueAtTime(gain, t0 + 0.01);
  g.gain.exponentialRampToValueAtTime(0.0001, t0 + dur);
  osc.connect(g).connect(ctx.destination);
  osc.start(t0);
  osc.stop(t0 + dur + 0.02);
}

export const sound = {
  start() {
    tone(660, 0, 0.08);
    tone(990, 0.07, 0.1);
  },
  complete() {
    tone(784, 0, 0.09);
    tone(1047, 0.08, 0.09);
    tone(1319, 0.16, 0.18);
  },
  critical() {
    [880, 1109, 1319, 1760].forEach((f, i) => tone(f, i * 0.06, 0.14, "sawtooth", 0.04));
  },
  mission() {
    [523, 659, 784, 1047, 784, 1047].forEach((f, i) => tone(f, i * 0.11, 0.22, "triangle", 0.07));
  },
  alert() {
    tone(440, 0, 0.15, "sawtooth", 0.04);
    tone(330, 0.18, 0.2, "sawtooth", 0.04);
  },
};
