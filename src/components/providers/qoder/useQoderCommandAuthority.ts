import { useLayoutEffect, useRef } from "react";

/** A command may consume only the authorization snapshot from the committed VM.
 * This is a local lifetime guard, never a backend generation or revision.
 */
export function useQoderCommandAuthority() {
  const frame = {};
  const current = useRef<object | null>(null);
  useLayoutEffect(() => {
    current.current = frame;
    return () => {
      if (current.current === frame) current.current = null;
    };
  });
  return {
    isCurrent: () => current.current === frame,
    invalidate: () => {
      if (current.current === frame) current.current = null;
    },
  };
}
