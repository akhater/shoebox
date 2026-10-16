/**
 * The same component, counting each time it is drawn. A `memo` component
 * stays one: only the function inside is wrapped, so its comparison of props
 * still decides whether it is drawn again.
 */
export function counted<C>(component: C, tick: () => void): C {
  type Render = (props: object) => unknown;
  const c = component as unknown as Render | { type: Render };
  if (typeof c === "function")
    return ((props: object) => {
      tick();
      return c(props);
    }) as C;
  return {
    ...c,
    type: (props: object) => {
      tick();
      return c.type(props);
    },
  } as C;
}
