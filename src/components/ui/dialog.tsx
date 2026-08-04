import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

const Dialog = DialogPrimitive.Root;

const DialogTrigger = DialogPrimitive.Trigger;

const DialogPortal = DialogPrimitive.Portal;

const DialogClose = DialogPrimitive.Close;

const DialogOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay> & {
    zIndex?: "base" | "nested" | "alert" | "top";
  }
>(({ className, zIndex = "base", ...props }, ref) => {
  const zIndexMap = {
    base: "z-40",
    nested: "z-50",
    alert: "z-[60]",
    top: "z-[110]",
  };

  return (
    <DialogPrimitive.Overlay
      ref={ref}
      className={cn(
        "fixed inset-0 bg-black/50 backdrop-blur-sm data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
        zIndexMap[zIndex],
        className,
      )}
      {...props}
    />
  );
});
DialogOverlay.displayName = DialogPrimitive.Overlay.displayName;

const DialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content> & {
    zIndex?: "base" | "nested" | "alert" | "top";
    variant?: "default" | "fullscreen";
    overlayClassName?: string;
    /**
     * 右上角那个 X。**默认给**（shadcn 官方模板本来就带，这个 fork 当初去掉了）。
     *
     * 只有两种情况该传 `false`：弹窗自己在别处已经放了关闭控件（`SessionToc` 的
     * header 里有一个、`BasicFormFields` 的图标选择器用返回箭头当关闭），
     * 那时再加一个就是两个紧邻控件做同一件事。
     *
     * ⚠️ **不要为「这一步不该让用户跳过」而传 false** —— 那个诉求该用「关掉之后
     * 还能从别处进来」满足，而不是靠堵死出口。本仓 `onInteractOutside` 已经
     * `preventDefault()`（不让点遮罩关），X 没了就只剩 Esc 这个隐藏操作。
     */
    showCloseButton?: boolean;
    /**
     * X 的无障碍标签。默认取 `common.close`（四个 locale 都有），
     * 只在某个弹窗需要更具体的措辞时才传。
     */
    closeButtonLabel?: string;
  }
>(
  (
    {
      className,
      children,
      zIndex = "base",
      variant = "default",
      overlayClassName,
      showCloseButton = true,
      closeButtonLabel,
      ...props
    },
    ref,
  ) => {
    // ⚠️ **走 `useTranslation` 而不是直接 import `@/i18n`**：后者会把 i18n 的
    // 初始化（`i18n.use(initReactI18next).init(...)`）拖进这个模块的依赖图，
    // 而仓里有 10 个测试文件 `vi.mock("react-i18next")` 且不导出
    // `initReactI18next` ⇒ 凡渲染任何弹窗的测试当场崩在 `src/i18n/index.ts:79`。
    // 实测踩过（4 个测试文件同时红）。这是本目录第一个用 hook 的组件，
    // 破例的理由就是它：mock 提供 `useTranslation`，不提供那个初始化导出。
    const { t } = useTranslation();
    const zIndexMap = {
      base: "z-40",
      nested: "z-50",
      alert: "z-[60]",
      top: "z-[110]",
    };

    const variantClass = {
      default:
        "fixed left-1/2 top-1/2 flex flex-col w-full max-w-lg max-h-[90vh] translate-x-[-50%] translate-y-[-50%] border border-border-default bg-background text-foreground shadow-lg duration-200 data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[state=closed]:slide-out-to-left-1/2 data-[state=closed]:slide-out-to-top-[48%] data-[state=open]:slide-in-from-left-1/2 data-[state=open]:slide-in-from-top-[48%] sm:rounded-lg",
      fullscreen:
        "fixed inset-0 flex flex-col w-screen h-screen translate-x-0 translate-y-0 bg-background text-foreground p-0 sm:rounded-none shadow-none",
    }[variant];

    return (
      <DialogPortal>
        <DialogOverlay zIndex={zIndex} className={overlayClassName} />
        <DialogPrimitive.Content
          ref={ref}
          className={cn(variantClass, zIndexMap[zIndex], className)}
          onInteractOutside={(e) => {
            // 防止点击遮罩层关闭对话框
            e.preventDefault();
          }}
          {...props}
        >
          {children}
          {/* 放在 children **之后**：它是绝对定位的，DOM 顺序只影响 Tab 焦点次序 ——
              排在末尾才不会抢在弹窗主内容之前拿到焦点。

              `z-10` 是必需的：`DialogHeader` 带 `bg-muted/20` 背景，同层级下
              后面的兄弟节点会盖住它，但 header 是 flex 布局里的实体块 ——
              不提层的话 X 会被 header 的背景吃掉一半。样式抄 `SessionToc` 那份
              （仓里已有的同形范例，CLAUDE.md §一「视觉 token 抄上游」）。 */}
          {showCloseButton && (
            <DialogPrimitive.Close
              className="absolute right-4 top-4 z-10 rounded-full p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus:outline-none focus:ring-2 focus:ring-primary focus:ring-offset-2 disabled:pointer-events-none"
              aria-label={closeButtonLabel ?? t("common.close")}
            >
              <X className="size-4" />
            </DialogPrimitive.Close>
          )}
        </DialogPrimitive.Content>
      </DialogPortal>
    );
  },
);
DialogContent.displayName = DialogPrimitive.Content.displayName;

const DialogHeader = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      "flex flex-col space-y-1.5 text-center sm:text-left px-6 py-5 border-b border-border-default bg-muted/20 flex-shrink-0",
      className,
    )}
    {...props}
  />
);
DialogHeader.displayName = "DialogHeader";

const DialogFooter = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end sm:items-center px-6 py-5 border-t border-border-default bg-muted/20 flex-shrink-0",
      className,
    )}
    {...props}
  />
);
DialogFooter.displayName = "DialogFooter";

const DialogTitle = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Title>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Title
    ref={ref}
    className={cn(
      "text-lg font-semibold leading-tight tracking-tight",
      className,
    )}
    {...props}
  />
));
DialogTitle.displayName = DialogPrimitive.Title.displayName;

const DialogDescription = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Description>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Description
    ref={ref}
    className={cn("text-sm text-muted-foreground", className)}
    {...props}
  />
));
DialogDescription.displayName = DialogPrimitive.Description.displayName;

export {
  Dialog,
  DialogTrigger,
  DialogContent,
  DialogHeader,
  DialogFooter,
  DialogTitle,
  DialogDescription,
  DialogClose,
};
