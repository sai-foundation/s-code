export function ownsRenderedItem<T>(
  itemsById: ReadonlyMap<string, T>,
  itemId: string,
  candidate: T,
): boolean {
  return itemsById.get(itemId) === candidate;
}
