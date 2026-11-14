// Sonification: turns a time series into a short melody with the Web Audio API (no library).
// Higher value = higher note, on a pentatonic scale so it always sounds musical.
// `bright` (0..1 per step, optional) opens a filter: e.g. more vegetation = brighter tone.

const SCALE = [0, 2, 4, 7, 9];
const BASE_HZ = 146.83; // D3
let ctx = null;
let timer = null;

export function stop() {
  clearInterval(timer);
  timer = null;
}

export function isPlaying() {
  return timer !== null;
}

export function play(values, { stepMs = 340, bright = null, onStep, onEnd } = {}) {
  stop();
  ctx = ctx || new (window.AudioContext || window.webkitAudioContext)();
  ctx.resume();
  const min = Math.min(...values);
  const span = Math.max(...values) - min || 1;
  let i = 0;
  const tick = () => {
    if (i >= values.length) {
      stop();
      onEnd?.();
      return;
    }
    const degree = Math.round(((values[i] - min) / span) * 12);
    const semis = 12 * Math.floor(degree / 5) + SCALE[degree % 5];
    note(BASE_HZ * 2 ** (semis / 12), (stepMs / 1000) * 2.2, bright ? bright[i] : 0.5);
    onStep?.(i);
    i += 1;
  };
  tick();
  timer = setInterval(tick, stepMs);
}

function note(freq, dur, brightness) {
  const t = ctx.currentTime;
  const gain = ctx.createGain();
  const filter = ctx.createBiquadFilter();
  filter.type = 'lowpass';
  filter.frequency.value = 500 + 3800 * Math.max(0, Math.min(1, brightness));
  gain.gain.setValueAtTime(0.0001, t);
  gain.gain.exponentialRampToValueAtTime(0.22, t + 0.02);
  gain.gain.exponentialRampToValueAtTime(0.0001, t + dur);
  filter.connect(gain).connect(ctx.destination);
  for (const [type, mult, level] of [['triangle', 1, 1], ['sine', 0.5, 0.6], ['sine', 2, 0.15]]) {
    const osc = ctx.createOscillator();
    const g = ctx.createGain();
    osc.type = type;
    osc.frequency.value = freq * mult;
    g.gain.value = level;
    osc.connect(g).connect(filter);
    osc.start(t);
    osc.stop(t + dur + 0.05);
  }
}
