// A small WebGL viewer for STL meshes: flat shading with a headlight, orbit and zoom.
// Z is up (the usual orientation of printable models).
import type { StlMesh } from "./stl";

type Mat4 = Float32Array;

const VERT = `
attribute vec3 p;
attribute vec3 n;
uniform mat4 mvp;
varying vec3 vn;
void main() { vn = n; gl_Position = mvp * vec4(p, 1.0); }`;

const FRAG = `
precision mediump float;
uniform vec3 light;
varying vec3 vn;
void main() {
  float d = min(1.0, abs(dot(normalize(vn), normalize(light))));
  vec3 base = vec3(0.36, 0.55, 0.92);
  gl_FragColor = vec4(base * (0.28 + 0.72 * d), 1.0);
}`;

export function perspective(fovY: number, aspect: number, near: number, far: number): Mat4 {
  const f = 1 / Math.tan(fovY / 2);
  const m = new Float32Array(16);
  m[0] = f / aspect; m[5] = f; m[10] = (far + near) / (near - far); m[11] = -1; m[14] = (2 * far * near) / (near - far);
  return m;
}

export function lookAt(eye: number[], up: number[]): Mat4 {
  // Looks at the origin.
  const norm = (v: number[]) => { const l = Math.hypot(v[0], v[1], v[2]) || 1; return [v[0] / l, v[1] / l, v[2] / l]; };
  const cross = (a: number[], b: number[]) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  const z = norm(eye);
  const x = norm(cross(up, z));
  const y = cross(z, x);
  const m = new Float32Array(16);
  m[0] = x[0]; m[4] = x[1]; m[8] = x[2];
  m[1] = y[0]; m[5] = y[1]; m[9] = y[2];
  m[2] = z[0]; m[6] = z[1]; m[10] = z[2];
  m[12] = -(x[0] * eye[0] + x[1] * eye[1] + x[2] * eye[2]);
  m[13] = -(y[0] * eye[0] + y[1] * eye[1] + y[2] * eye[2]);
  m[14] = -(z[0] * eye[0] + z[1] * eye[1] + z[2] * eye[2]);
  m[15] = 1;
  return m;
}

export function multiply(a: Mat4, b: Mat4): Mat4 {
  const out = new Float32Array(16);
  for (let c = 0; c < 4; c++)
    for (let r = 0; r < 4; r++) {
      let s = 0;
      for (let k = 0; k < 4; k++) s += a[k * 4 + r] * b[c * 4 + k];
      out[c * 4 + r] = s;
    }
  return out;
}

const HOME = { yaw: -0.7, pitch: 0.5, dist: 3.4 };

export interface StlViewer {
  /** Rotates by pixel deltas of a drag. */
  rotate(dx: number, dy: number): void;
  /** Positive zooms out. */
  zoom(delta: number): void;
  reset(): void;
  resize(): void;
  dispose(): void;
}

/** Throws if WebGL is not available. */
export function createStlViewer(canvas: HTMLCanvasElement, mesh: StlMesh): StlViewer {
  const gl = canvas.getContext("webgl", { antialias: true, alpha: true });
  if (!gl) throw new Error("WebGL is not available");
  const shader = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader error");
    return s;
  };
  const prog = gl.createProgram()!;
  gl.attachShader(prog, shader(gl.VERTEX_SHADER, VERT));
  gl.attachShader(prog, shader(gl.FRAGMENT_SHADER, FRAG));
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? "link error");
  gl.useProgram(prog);

  // Centre the model and scale it to a unit sphere so any size fits the same camera.
  const c = [0, 1, 2].map((i) => (mesh.min[i] + mesh.max[i]) / 2);
  const radius = Math.hypot(mesh.max[0] - mesh.min[0], mesh.max[1] - mesh.min[1], mesh.max[2] - mesh.min[2]) / 2 || 1;
  const pos = new Float32Array(mesh.positions.length);
  for (let i = 0; i < pos.length; i++) pos[i] = (mesh.positions[i] - c[i % 3]) / radius;

  const attr = (name: string, data: Float32Array) => {
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(prog, name);
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 3, gl.FLOAT, false, 0, 0);
  };
  attr("p", pos);
  attr("n", mesh.normals);
  const uMvp = gl.getUniformLocation(prog, "mvp");
  const uLight = gl.getUniformLocation(prog, "light");
  gl.enable(gl.DEPTH_TEST);
  gl.clearColor(0, 0, 0, 0);

  let { yaw, pitch, dist } = HOME;
  let frame = 0;
  const draw = () => {
    frame = 0;
    const cp = Math.cos(pitch);
    const eye = [cp * Math.cos(yaw) * dist, cp * Math.sin(yaw) * dist, Math.sin(pitch) * dist];
    const aspect = canvas.width / Math.max(1, canvas.height);
    const mvp = multiply(perspective(0.7, aspect, 0.1, 50), lookAt(eye, [0, 0, 1]));
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.uniformMatrix4fv(uMvp, false, mvp);
    const l = Math.hypot(eye[0], eye[1], eye[2]);
    gl.uniform3f(uLight, eye[0] / l, eye[1] / l, eye[2] / l + 0.3);
    gl.drawArrays(gl.TRIANGLES, 0, mesh.triangles * 3);
  };
  const redraw = () => { if (!frame) frame = requestAnimationFrame(draw); };
  const resize = () => {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
    redraw();
  };
  resize();
  return {
    rotate(dx, dy) {
      yaw -= dx * 0.01;
      pitch = Math.max(-1.5, Math.min(1.5, pitch + dy * 0.01));
      redraw();
    },
    zoom(delta) {
      dist = Math.max(1.6, Math.min(12, dist * Math.exp(delta * 0.0015)));
      redraw();
    },
    reset() { ({ yaw, pitch, dist } = HOME); redraw(); },
    resize,
    dispose() {
      if (frame) cancelAnimationFrame(frame);
      // The context is left to be collected with the canvas: losing it here would break the
      // second effect run of React's StrictMode, which reuses the same canvas.
    },
  };
}
