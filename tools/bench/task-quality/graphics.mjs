// 题面明确约定的像素位置与行为。判据只消费浏览器实际帧，不推断 Three.js
// 等引擎的源码写法；比如遮罩和混合可以同时存在，合法双面也不应被扣分。
const BACKGROUND = 0x172235;
const LEAF = 0x45b567;
const FACE = 0x4e9ceb;
const texels = (pixel, origin) => Array.from({ length: 64 }, (_, i) =>
  pixel(origin + (i % 8) * 4 + 2, Math.floor(i / 8) * 4 + 2));

function differentTiles(pixel, back) {
  const grass = texels(pixel, 0);
  const rock = texels(pixel, 32);
  const otherGrass = texels(back, 0);
  const otherRock = texels(back, 32);
  return new Set(grass).size >= 4 && new Set(rock).size >= 4
    && grass.filter((color, i) => color !== rock[i]).length >= 32
    && grass.every((color, i) => color >= 0 && color === otherGrass[i] && rock[i] >= 0 && rock[i] === otherRock[i]);
}

function crispGrid(pixel, back) {
  for (let y = 0; y < 32; y++) {
    for (let x = 0; x < 64; x++) {
      const color = pixel(x, y);
      if (color < 0 || color !== pixel(x - x % 4, y - y % 4) || color !== back(x, y)) return false;
    }
  }
  return true;
}

function swatch(pixel, x, width, color) {
  for (let y = 0; y < 16; y++) {
    for (let dx = 0; dx < width; dx++) if (pixel(x + dx, y) !== color) return false;
  }
  return true;
}

export const terrainSpec = Object.freeze([
  { id: 'alpha-cutout', check: (front) => swatch(front, 72, 8, LEAF) && swatch(front, 80, 8, BACKGROUND) },
  { id: 'backface-cull', check: (front, back) => swatch(front, 96, 16, FACE) && swatch(back, 96, 16, BACKGROUND) },
  { id: 'ao-shading', check: (front) => {
    const colors = [front(132, 8), front(124, 8)];
    if (colors.some((rgb) => rgb < 0 || (rgb >> 16) !== (rgb & 255) || ((rgb >> 8) & 255) !== (rgb & 255))) return false;
    const [open, corner] = colors.map((rgb) => rgb & 255);
    return open >= 100 && corner >= 32 && open - corner >= 24;
  } },
  { id: 'texture-diversity', check: differentTiles },
  { id: 'nearest-sampling', check: crispGrid },
]);

export function gradeGraphics({ front, back }, requirements) {
  return Object.fromEntries(requirements.map(({ id, check }) => [id, check(front, back)]));
}
