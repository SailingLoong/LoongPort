import { useState } from "react";
import { createRoot } from "react-dom/client";
import { Button } from "@/components/ui/button";
import { Notice, NoticeSlot } from "@/components/ui/notice";
import { HelpTip, DisabledReason } from "@/components/ui/help-tip";
import { PageTabs } from "@/components/ui/page-tabs";
import { SearchField } from "@/components/ui/search-field";
import { SegmentedControl } from "@/components/ui/segmented-control";
import {
  Sheet,
  SheetTrigger,
  SheetContent,
  SheetTitle,
  SheetDescription,
  SheetHeader,
  SheetBody,
  SheetFooter,
  SheetClose,
} from "@/components/ui/sheet";
import { PreservedView } from "@/components/ui/PreservedView";
import { initializeInputModality } from "@/lib/inputModality";
import "@/index.css";

// Synthetic component-only fixture. It does not import APIs or production data.
initializeInputModality();
const params = new URLSearchParams(location.search);
if (params.get("dark") === "true")
  document.documentElement.classList.add("dark");

function FoundationFixture() {
  const [tab, setTab] = useState("connection");
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [notice, setNotice] = useState(true);
  const [actions, setActions] = useState(0);
  const [active, setActive] = useState(true);
  const [draft, setDraft] = useState("");
  const [drawerSearch, setDrawerSearch] = useState("populated drawer search");
  return (
    <>
      <div
        data-native-chrome
        className="fixed inset-x-0 top-0 z-[70] h-7 bg-subtle text-center text-caption"
      >
        Synthetic native chrome
      </div>
      <main className="page-content space-y-6 pt-12">
        <h1 className="text-page">UI foundation</h1>
        <PageTabs
          aria-label="Preview sections"
          idPrefix="preview"
          controls="preview-panel"
          value={tab}
          onValueChange={setTab}
          items={[
            { value: "connection", label: "Connection" },
            { value: "blocked", label: "Unavailable", disabled: true },
            { value: "files", label: "Files" },
          ]}
        />
        <div
          role="tabpanel"
          id="preview-panel"
          aria-labelledby={`preview-${tab}`}
          className="text-body"
        >
          Synthetic {tab} preview
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <Button
            data-primary
            size="compact"
            onClick={() => setActions((count) => count + 1)}
          >
            Apply example
          </Button>
          <Button variant="neutral" size="compact">
            Cancel example
          </Button>
          <Button variant="destructive" size="compact">
            Delete example
          </Button>
          <Button disabled size="compact">
            Disabled example
          </Button>
          <Button disabled aria-busy size="compact">
            Loading example
          </Button>
          <HelpTip title="What is a draft?">
            Changes remain local until applied.
          </HelpTip>
          <DisabledReason reason="Synthetic unavailable state.">
            <Button onClick={() => setActions((count) => count + 1)}>
              Blocked example
            </Button>
          </DisabledReason>
        </div>
        <SearchField
          aria-label="Search tiers"
          clearLabel="Clear search"
          value={search}
          onValueChange={setSearch}
        />
        <SegmentedControl
          aria-label="Tier filter"
          value={filter}
          onValueChange={setFilter}
          items={[
            { value: "all", label: "All" },
            { value: "available", label: "Available" },
          ]}
        />
        <NoticeSlot>
          {notice && (
            <Notice
              title="Changes have not been applied"
              tone="warning"
              actions={
                <Button
                  size="compact"
                  onClick={() => setActions((count) => count + 1)}
                >
                  Review example
                </Button>
              }
              onDismiss={() => setNotice(false)}
              dismissLabel="Dismiss notice"
            >
              Synthetic draft only.
            </Notice>
          )}
        </NoticeSlot>
        <p data-action-count={actions} className="text-caption text-fg-2">
          Action count: {actions}
        </p>
        <Button variant="quiet" onClick={() => setActive(true)}>
          Return to draft page
        </Button>
        <PreservedView active={active}>
          <Sheet>
            <SheetTrigger asChild>
              <Button variant="neutral">Open draft drawer</Button>
            </SheetTrigger>
            <SheetContent closeLabel="Close draft drawer">
              <SheetHeader>
                <SheetTitle>Draft options</SheetTitle>
                <SheetDescription>
                  Local synthetic values only.
                </SheetDescription>
              </SheetHeader>
              <SheetBody>
                <SearchField
                  aria-label="Search in drawer"
                  clearLabel="Clear drawer search"
                  value={drawerSearch}
                  onValueChange={setDrawerSearch}
                />
                <SearchField
                  aria-label="Read-only search"
                  clearLabel="Clear read-only search"
                  value="protected value"
                  onValueChange={() => setActions((count) => count + 1)}
                  readOnly
                />
                <SearchField
                  aria-label="Disabled search"
                  clearLabel="Clear disabled search"
                  value="protected value"
                  onValueChange={() => setActions((count) => count + 1)}
                  disabled
                />
                <label className="text-body">
                  Draft value
                  <input
                    aria-label="Draft value"
                    className="mt-2 w-full border border-border p-2"
                    value={draft}
                    onChange={(event) => setDraft(event.target.value)}
                  />
                </label>
                <div className="mt-4 flex items-center gap-2">
                  <span>Help in drawer</span>
                  <HelpTip title="Drawer explanation">
                    This explanation stays above the drawer.
                  </HelpTip>
                </div>
              </SheetBody>
              <SheetFooter>
                <Button variant="quiet" onClick={() => setActive(false)}>
                  Visit another page
                </Button>
                <SheetClose asChild>
                  <Button variant="neutral">Cancel drawer</Button>
                </SheetClose>
              </SheetFooter>
            </SheetContent>
          </Sheet>
        </PreservedView>
      </main>
    </>
  );
}
createRoot(document.getElementById("root")!).render(<FoundationFixture />);
