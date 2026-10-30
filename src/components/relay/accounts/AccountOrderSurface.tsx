import { createContext, useContext, type ReactNode } from "react";
import { GripVertical } from "lucide-react";
import {
  DndContext,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import { SortableContext, useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useTranslation } from "react-i18next";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Button } from "@/components/ui/button";
import type { AccountPlatform } from "./accountPlatforms";
export interface AccountOrderItem {
  id: AccountPlatform;
  label: string;
  previous?: AccountPlatform;
  next?: AccountPlatform;
}
interface OrderProps {
  items: readonly AccountOrderItem[];
  disabled?: boolean;
  onReorder: (id: AccountPlatform, over: AccountPlatform) => void;
}
/** Display order only. No account, provider or routing priority writes. */
export function AccountOrderSurface({
  items,
  disabled,
  onReorder,
  children,
}: OrderProps & { children: ReactNode }) {
  const { t } = useTranslation();
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
  );
  return (
    <DndContext
      sensors={sensors}
      accessibility={{
        screenReaderInstructions: {
          draggable: t("accountsOverview.dragInstructions"),
        },
      }}
      collisionDetection={closestCenter}
      onDragEnd={({ active, over }) => {
        if (disabled || !over || active.id === over.id) return;
        const source = items.find((item) => item.id === active.id);
        const target = items.find((item) => item.id === over.id);
        if (source && target) onReorder(source.id, target.id);
      }}
    >
      <SortableContext items={items.map((item) => item.id)}>
        {children}
      </SortableContext>
    </DndContext>
  );
}
const DragHandleContext = createContext<ReturnType<typeof useSortable> | null>(
  null,
);
export function AccountDragHandle({
  label,
  disabled,
}: {
  label: string;
  disabled?: boolean;
}) {
  const sortable = useContext(DragHandleContext);
  return (
    <Button
      ref={sortable?.setActivatorNodeRef}
      variant="ghost"
      size="sm"
      disabled={disabled}
      aria-label={label}
      {...sortable?.attributes}
      {...sortable?.listeners}
    >
      <GripVertical className="h-4 w-4" />
    </Button>
  );
}
export function SortableAccountGroup({
  id,
  disabled,
  children,
  className,
  handleOnly = false,
}: {
  id: AccountPlatform;
  disabled?: boolean;
  children: ReactNode;
  className?: string;
  handleOnly?: boolean;
}) {
  const sortable = useSortable({ id, disabled });
  const { setNodeRef, transform, transition, attributes, listeners } = sortable;
  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      className={className}
      {...(!handleOnly ? attributes : {})}
      {...(!handleOnly ? listeners : {})}
      role={undefined}
      tabIndex={undefined}
    >
      <DragHandleContext.Provider value={sortable}>
        {children}
      </DragHandleContext.Provider>
    </div>
  );
}
export function AccountOrderMenu({
  items,
  onReorder,
  disabled,
  vertical = false,
  label,
}: OrderProps & { vertical?: boolean; label: string }) {
  const { t } = useTranslation();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          disabled={disabled}
          aria-label={label}
        >
          {t("accountsOverview.sort")}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent>
        {items.map((item) => (
          <div key={item.id}>
            <DropdownMenuItem
              disabled={!item.previous || disabled}
              onSelect={() =>
                item.previous && onReorder(item.id, item.previous)
              }
            >
              {item.label}：
              {t(
                vertical
                  ? "accountsOverview.moveUp"
                  : "accountsOverview.moveLeft",
              )}
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={!item.next || disabled}
              onSelect={() => item.next && onReorder(item.id, item.next)}
            >
              {item.label}：
              {t(
                vertical
                  ? "accountsOverview.moveDown"
                  : "accountsOverview.moveRight",
              )}
            </DropdownMenuItem>
          </div>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
