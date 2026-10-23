/**
 * Lightweight safe implementation of hideOthers for aria-hidden accessibility
 */
export function hideOthers(
  elements: HTMLElement[] | HTMLElement,
  parentNode: HTMLElement = document.body,
): () => void {
  const targets = Array.isArray(elements) ? elements : [elements];
  const hiddenElements: HTMLElement[] = [];

  const walk = (node: HTMLElement) => {
    for (let i = 0; i < node.children.length; i++) {
      const child = node.children[i] as HTMLElement;
      if (!child?.getAttribute) continue;

      const containsTarget = targets.some(
        (target) => target === child || child.contains(target),
      );

      if (!containsTarget) {
        const alreadyHidden = child.getAttribute("aria-hidden");
        if (alreadyHidden !== "true") {
          child.setAttribute("aria-hidden", "true");
          hiddenElements.push(child);
        }
      } else if (targets.every((t) => t !== child)) {
        walk(child);
      }
    }
  };

  try {
    walk(parentNode);
  } catch {
    // Ignore in non-DOM environments
  }

  return () => {
    hiddenElements.forEach((el) => {
      try {
        el.removeAttribute("aria-hidden");
      } catch {
        // Safe ignore
      }
    });
  };
}
