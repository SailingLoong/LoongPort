import {
  Fragment,
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { Search, User, ArrowRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import type { AppId } from "@/lib/api/types";

export const AccountApplicationNavigation = createContext<
  ((targetApp?: AppId) => void) | undefined
>(undefined);

/** A navigation request only; selection and account state stay in the native workspace. */
export const AccountWorkspaceTarget = createContext<
  { id: string; request: number } | undefined
>(undefined);

/** Shared presentation, not an account store: every detail/action retains its native owner. */
export function AccountWorkspace<T extends { id: string }>({
  accounts,
  label,
  identityDetails,
  children,
  enabled = true,
}: {
  accounts: T[];
  label: (row: T) => string;
  identityDetails?: (row: T) => ReactNode;
  children: (row: T) => ReactNode;
  enabled?: boolean;
}) {
  const openApplication = useContext(AccountApplicationNavigation);
  const target = useContext(AccountWorkspaceTarget);
  const [selected, setSelected] = useState<string | undefined>(target?.id);
  useEffect(() => {
    if (target) setSelected(target.id);
  }, [target]);
  const [search, setSearch] = useState("");
  if (!enabled)
    return (
      <>
        {accounts.map((row) => (
          <Fragment key={row.id}>{children(row)}</Fragment>
        ))}
      </>
    );
  const current =
    accounts.find((row) => row.id === selected) ??
    (target ? undefined : accounts[0]);
  const filtered = accounts.filter((row) =>
    label(row).toLocaleLowerCase().includes(search.toLocaleLowerCase()),
  );
  return (
    <div className="grid min-h-64 overflow-hidden rounded-xl border border-border bg-card md:grid-cols-[15rem_minmax(0,1fr)]">
      <aside
        aria-label="账号列表"
        className="border-b border-border bg-muted/20 p-3 md:border-b-0 md:border-r"
      >
        <div className="relative mb-3">
          <Search
            aria-hidden="true"
            className="absolute left-2 top-2.5 h-4 w-4 text-muted-foreground"
          />
          <Input
            type="search"
            aria-label="搜索账号"
            placeholder="搜索账号"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            className="pl-8"
          />
        </div>
        <div className="space-y-1">
          {filtered.map((row) => (
            <button
              key={row.id}
              type="button"
              aria-label={`查看 ${label(row)}`}
              aria-pressed={current?.id === row.id}
              onClick={() => setSelected(row.id)}
              className={cn(
                "flex w-full items-center gap-2 rounded-lg p-3 text-left text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                current?.id === row.id
                  ? "bg-primary/10 text-primary"
                  : "hover:bg-muted",
              )}
            >
              <User className="h-4 w-4 shrink-0" />
              <span className="min-w-0 break-all">{label(row)}</span>
            </button>
          ))}
        </div>
        {!filtered.length && (
          <p className="p-3 text-xs text-muted-foreground">
            {accounts.length ? "没有匹配的账号" : "此平台还没有账号"}
          </p>
        )}
      </aside>
      <section aria-label="账号详情" className="min-w-0 space-y-4 p-4">
        {current ? (
          <>
            <header className="border-b border-border pb-3">
              <p className="text-xs text-muted-foreground">账号身份</p>
              <h3 className="mt-1 break-all font-semibold">{label(current)}</h3>
              <p className="mt-1 break-all text-xs text-muted-foreground">
                账号 ID：{current.id}
              </p>
              {identityDetails?.(current)}
            </header>
            {children(current)}
            {openApplication && (
              <AccountDetailSection title="用于哪些应用">
                <p className="text-xs text-muted-foreground">
                  应用配置引用原账号。配置已保存不等于推理已接通，实际运行仍需验证。
                </p>
                <Button variant="outline" onClick={() => openApplication()}>
                  到应用中查看与配置
                  <ArrowRight className="ml-2 h-4 w-4" />
                </Button>
              </AccountDetailSection>
            )}
          </>
        ) : (
          <p className="text-sm text-muted-foreground">
            添加账号后查看授权、权益与应用使用。
          </p>
        )}
      </section>
    </div>
  );
}
export function AccountDetailSection({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className="space-y-3 rounded-lg border border-border p-4"
    >
      <h4 className="text-sm font-medium">{title}</h4>
      {children}
    </section>
  );
}

/** Stable semantic sections. Slots contain only projections and actions of the native owner. */
export function AccountDetailSections({
  authorization,
  entitlements,
  inference,
}: {
  authorization: ReactNode;
  entitlements: ReactNode;
  inference: ReactNode;
}) {
  return (
    <div className="space-y-4">
      <AccountDetailSection title="授权状态">
        {authorization}
      </AccountDetailSection>
      <AccountDetailSection title="权益与签到">
        {entitlements}
      </AccountDetailSection>
      <AccountDetailSection title="模型与推理">
        {inference}
      </AccountDetailSection>
    </div>
  );
}
