import { useState } from "react";
import { Button } from "@/components/ui/button";
import { WorkBuddyNativeAttachment } from "@/components/workbuddy/WorkBuddyNativeAttachment";
import type { WorkBuddyNativeAttachmentView as View } from "@/lib/api/workbuddy";

// Synthetic transport only. No credential, filesystem, client or native writer.
let current: View | null = null;
let lose: (() => void) | undefined;
let late: (() => void) | undefined;
const saved = (): View => ({
  ...current!,
  state: "saved",
  expiresInMs: 0,
  saved: {
    selection: {
      id: current!.account!.id,
      generation: 1,
      incarnation: "00000000-0000-4000-8000-000000000001",
    },
    label: current!.account!.label,
    realm: current!.account!.realm,
  },
});
export const attachment = {
  confirms: 0,
  defer: false,
  loseResponse: () => {
    if (!lose) throw new Error("No pending synthetic response");
    lose();
    lose = undefined;
  },
  releaseLateSaved: () => {
    if (!late) throw new Error("No pending synthetic inspection");
    late();
    late = undefined;
  },
};
export function attachmentIPC(command: string, payload: unknown): unknown {
  const args = payload as { id?: string; flowId?: string | null };
  switch (command) {
    case "prepare_workbuddy_native_attachment": {
      if (!args?.id || !["attachment-a", "attachment-b"].includes(args.id))
        throw new Error("Invalid synthetic id");
      current = {
        flowId: `synthetic-${args.id}`,
        available: true,
        state: "ready",
        account: {
          id: args.id,
          label: `Synthetic attachment ${args.id.endsWith("a") ? "A" : "B"}`,
          realm: "www.workbuddy.cn",
        },
        saved: null,
        reason: null,
        expiresInMs: 300000,
      };
      return current;
    }
    case "confirm_workbuddy_native_attachment": {
      if (
        !current ||
        args?.flowId !== current.flowId ||
        current.state !== "ready"
      )
        throw new Error("Synthetic duplicate confirmation");
      attachment.confirms++;
      if (!attachment.defer) {
        current = saved();
        return current;
      }
      current = { ...current, state: "applying", expiresInMs: 0 };
      return new Promise((_resolve, reject) => {
        lose = () => {
          current = { ...current!, state: "unknown", saved: null };
          reject(new Error("Synthetic lost response"));
        };
      });
    }
    case "inspect_workbuddy_native_attachment": {
      if (attachment.defer && current?.state === "unknown") {
        const receipt = saved();
        return new Promise((resolve) => {
          late = () => resolve(receipt);
        });
      }
      return current;
    }
    case "cancel_workbuddy_native_attachment":
      if (!current || args?.flowId !== current.flowId)
        throw new Error("Synthetic wrong flow");
      if (current.state === "preparing" || current.state === "ready") {
        current = {
          ...current,
          state: "cancelled",
          saved: null,
          expiresInMs: 0,
        };
      }
      return current;
    default:
      throw new Error(`Unexpected synthetic attachment command: ${command}`);
  }
}
export function AttachmentFixture() {
  const [account, setAccount] = useState("attachment-a");
  const [refreshed, setRefreshed] = useState(0);
  return (
    <main className="p-4 space-y-4">
      <p>Mock attachment receipt only. Real native writes remain rejected.</p>
      <Button onClick={() => setAccount("attachment-a")}>
        Select synthetic attachment A
      </Button>
      <Button onClick={() => setAccount("attachment-b")}>
        Select synthetic attachment B
      </Button>
      <output data-testid="attachment-refreshed">{refreshed}</output>
      <WorkBuddyNativeAttachment
        accountId={account}
        onAttached={() => setRefreshed((value) => value + 1)}
      />
    </main>
  );
}
