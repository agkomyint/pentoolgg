import { expect, test } from "bun:test";
import { applyCrop, cropLimits, dragCrop, fitRatio, pureOffset } from "./cropStore";

const frame = { x: 20, y: 10, width: 100, height: 60 };
const none = { dx: 0, dy: 0 };

test("an untouched crop rectangle keeps the frame and the window", () => {
  const out = applyCrop(frame, [0, 0, 1, 1], none, { x: 20, y: 10, width: 100, height: 60 });
  expect(out.frame).toEqual(frame);
  expect(out.crop).toEqual([0, 0, 1, 1]);
});

test("cropping inward keeps every pixel where it was on the page", () => {
  const out = applyCrop(frame, [0, 0, 1, 1], none, { x: 40, y: 10, width: 50, height: 30 });
  expect(out.frame).toEqual({ x: 40, y: 10, width: 50, height: 30 });
  expect(out.crop).toEqual([0.2, 0, 0.5, 0.5]);
});

test("the node offset (a group translation) is removed from the frame", () => {
  const out = applyCrop(frame, [0, 0, 1, 1], { dx: 100, dy: 0 }, { x: 140, y: 10, width: 50, height: 60 });
  expect(out.frame.x).toBe(40);
  expect(out.crop[0]).toBe(0.2);
});

test("a previous crop can be widened again, but not beyond the source", () => {
  const cropped = { x: 40, y: 10, width: 50, height: 30 };
  const window: [number, number, number, number] = [0.2, 0, 0.5, 0.5];
  const limits = cropLimits(cropped, window, none);
  expect(limits).toEqual({ l: 20, t: 10, r: 120, b: 70 });
  const wide = dragCrop({ x: 40, y: 10, width: 50, height: 30 }, "e", { x: 500, y: 20 }, limits, null);
  expect(wide.x + wide.width).toBe(120);
  const back = applyCrop(cropped, window, none, { x: 20, y: 10, width: 100, height: 60 });
  expect(back.crop).toEqual([0, 0, 1, 1]);
});

test("dragging keeps a ratio lock and never leaves the source", () => {
  const limits = { l: 0, t: 0, r: 200, b: 100 };
  const box = { x: 20, y: 20, width: 80, height: 40 };
  const out = dragCrop(box, "se", { x: 140, y: 400 }, limits, 2);
  expect(out.width / out.height).toBeCloseTo(2, 1);
  expect(out.y + out.height).toBeLessThanOrEqual(100);
  const side = dragCrop(box, "n", { x: 60, y: -50 }, limits, 1);
  expect(side.width / side.height).toBeCloseTo(1, 1);
  expect(side.y).toBeGreaterThanOrEqual(0);
});

test("fitRatio centers the largest rectangle of the ratio", () => {
  expect(fitRatio({ x: 0, y: 0, width: 100, height: 100 }, 2)).toEqual({ x: 0, y: 25, width: 100, height: 50 });
  expect(fitRatio({ x: 0, y: 0, width: 100, height: 40 }, 1)).toEqual({ x: 30, y: 0, width: 40, height: 40 });
});

test("rotated or scaled worlds are refused, translations are not", () => {
  expect(pureOffset([1, 0, 0, 1, 5, 6])).toEqual({ dx: 5, dy: 6 });
  expect(pureOffset([2, 0, 0, 1, 0, 0])).toBeNull();
  expect(pureOffset([0, 1, -1, 0, 0, 0])).toBeNull();
});
