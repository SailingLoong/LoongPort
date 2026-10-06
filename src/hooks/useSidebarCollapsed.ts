import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useState,
  type RefObject,
} from "react";

/** Upstream v4.0.2 shell preference owner, adapted to LoongPort's fixed shell.
 * Keep the seven-destination shell and native drag bar; the upstream compositor
 * cover assumes a different stacking context and is intentionally not mounted.
 */
export const SIDEBAR_AUTO_COLLAPSE_WIDTH = 960;
export const SIDEBAR_EXPANDED_WIDTH = 200;
export const sidebarRailWidth = () => 72;
export const sidebarWidth = (collapsed: boolean) =>
  collapsed ? sidebarRailWidth() : SIDEBAR_EXPANDED_WIDTH;

const STORAGE_KEY = "loongport-sidebar-collapsed";

function readManualPreference(): boolean | null {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "true") return true;
    if (saved === "false") return false;
  } catch {
    // An unavailable preference store must not prevent navigation.
  }
  return null;
}

function isNarrowWindow(): boolean {
  return (
    typeof window !== "undefined" &&
    window.innerWidth < SIDEBAR_AUTO_COLLAPSE_WIDTH
  );
}

export function useSidebarCollapsed(navRef: RefObject<HTMLElement | null>) {
  const [manual, setManual] = useState<boolean | null>(readManualPreference);
  const [viewport, setViewport] = useState(() => ({
    narrow: isNarrowWindow(),
    override: null as boolean | null,
  }));

  useEffect(() => {
    const onResize = () => {
      const narrow = isNarrowWindow();
      setViewport((previous) =>
        previous.narrow === narrow ? previous : { narrow, override: null },
      );
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // Each narrow visit starts as a rail, but keyboard/button expansion is still
  // available. A temporary narrow choice must not replace the wide preference.
  const collapsed = viewport.narrow
    ? (viewport.override ?? true)
    : (manual ?? false);

  useLayoutEffect(() => {
    const shell = navRef.current?.closest<HTMLElement>("[data-client-shell]");
    if (!shell) return;
    const previous = shell.style.getPropertyValue("--sidebar-width");
    shell.style.setProperty("--sidebar-width", `${sidebarWidth(collapsed)}px`);
    return () => {
      if (previous) shell.style.setProperty("--sidebar-width", previous);
      else shell.style.removeProperty("--sidebar-width");
    };
  }, [collapsed, navRef]);

  const toggle = useCallback(() => {
    const next = !collapsed;
    if (viewport.narrow) {
      setViewport((previous) => ({ ...previous, override: next }));
      return;
    }
    setManual(next);
    try {
      localStorage.setItem(STORAGE_KEY, String(next));
    } catch {
      // Persistence is optional; this interaction must still work.
    }
  }, [collapsed, viewport.narrow]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.repeat ||
        event.isComposing ||
        event.altKey ||
        event.shiftKey ||
        event.getModifierState("AltGraph")
      )
        return;
      if ((event.metaKey || event.ctrlKey) && event.key === "\\") {
        event.preventDefault();
        toggle();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [toggle]);

  return { collapsed, toggle };
}
