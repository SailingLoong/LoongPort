import {
  Activity,
  createContext,
  useContext,
  useState,
  type ReactNode,
} from "react";

const ActiveViewContext = createContext(true);

export function usePreservedViewActive() {
  return useContext(ActiveViewContext);
}

/** Retain in-memory drafts and suspend effects, including portal focus locks. */
export function PreservedView({
  active,
  children,
}: {
  active: boolean;
  children: ReactNode;
}) {
  const parentActive = useContext(ActiveViewContext);
  const visible = active && parentActive;
  const [visited, setVisited] = useState(visible);
  if (visible && !visited) setVisited(true);
  if (!visited && !visible) return null;
  return (
    <ActiveViewContext.Provider value={visible}>
      <Activity mode={visible ? "visible" : "hidden"}>{children}</Activity>
    </ActiveViewContext.Provider>
  );
}
