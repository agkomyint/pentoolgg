import { describe, expect, test } from "bun:test";
import { apply, invert, nodeWorldTransform, type Found } from "./model";

describe("nodeWorldTransform", () => {
  test("includes the raster node transform beneath transformed groups", () => {
    const found = {
      node: { id: "paint", kind: "raster", transform: [0, 2, -2, 0, 7, 11] },
      world: [1.5, 0, 0, 1.5, 20, 30],
    } as unknown as Pick<Found, "node" | "world">;
    const world = nodeWorldTransform(found);
    const pagePoint = apply(world, 15, 25);
    const restored = apply(invert(world)!, ...pagePoint);
    expect(restored[0]).toBeCloseTo(15);
    expect(restored[1]).toBeCloseTo(25);
    expect(pagePoint).toEqual([-44.5, 91.5]);
  });
});
