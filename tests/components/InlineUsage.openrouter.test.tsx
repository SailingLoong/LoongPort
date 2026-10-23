import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { InlineUsage } from "@/components/UsageFooter";
import type { UsageResult } from "@/types";

const explanation =
  "API key verified; account balance unavailable (management key required).";

function show(usage: UsageResult) {
  const onRefresh = vi.fn();
  const rendered = render(
    <InlineUsage
      usage={usage}
      loading={false}
      lastQueriedAt={null}
      onRefresh={onRefresh}
    />,
  );
  return { ...rendered, onRefresh };
}

describe("OpenRouter key verification is not an account balance", () => {
  it("renders the backend explanation without amounts or an expired state", () => {
    const { container, onRefresh } = show({
      success: true,
      data: [{ planName: "OpenRouter", extra: explanation }],
    });
    expect(screen.getByText(explanation)).toBeVisible();
    expect(screen.queryByText("usage.remaining")).not.toBeInTheDocument();
    expect(screen.queryByText("usage.used")).not.toBeInTheDocument();
    expect(screen.queryByText("usage.queryFailed")).not.toBeInTheDocument();
    expect(container.querySelector(".text-red-500")).toBeNull();
    expect(container.textContent).not.toMatch(
      /0\.00|USD|expired|Authentication failed/,
    );
    fireEvent.click(screen.getByRole("button"));
    expect(onRefresh).toHaveBeenCalledOnce();
  });

  it("still displays actual management-key credits", () => {
    show({
      success: true,
      data: [
        {
          planName: "OpenRouter",
          total: 100.5,
          used: 25.75,
          remaining: 74.75,
          unit: "USD",
          isValid: true,
        },
      ],
    });
    expect(screen.getByText("74.75")).toBeVisible();
    expect(screen.getByText("25.75")).toBeVisible();
    expect(screen.queryByText(explanation)).not.toBeInTheDocument();
  });

  it("retains a refreshable error for a genuinely invalid key", () => {
    const { onRefresh } = show({
      success: false,
      data: [
        { isValid: false, invalidMessage: "Authentication failed (HTTP 401)" },
      ],
      error: "Authentication failed (HTTP 401)",
    });
    expect(screen.getByText("usage.queryFailed")).toBeVisible();
    expect(screen.getByTitle("Authentication failed (HTTP 401)")).toBeVisible();
    fireEvent.click(screen.getByRole("button"));
    expect(onRefresh).toHaveBeenCalledOnce();
  });
});
