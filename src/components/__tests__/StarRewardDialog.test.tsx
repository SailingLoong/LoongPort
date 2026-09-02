import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { StarRewardOffer } from "@/lib/api";
import { StarRewardDialog } from "@/components/StarRewardDialog";

const { openRegisterWindow, openExternal, markClaimed } = vi.hoisted(() => ({
  openRegisterWindow: vi.fn(),
  openExternal: vi.fn(),
  markClaimed: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  settingsApi: { openExternal },
  starRewardApi: { openRegisterWindow, markClaimed },
}));
vi.mock("@/lib/clipboard", () => ({
  copyText: vi.fn().mockResolvedValue(undefined),
}));

const offer: StarRewardOffer = {
  promoCode: "LOONGPORT5",
  amountUsd: 5,
};

function renderDialog(onClaimed = vi.fn(), onClose = vi.fn()) {
  render(
    <StarRewardDialog offer={offer} onClose={onClose} onClaimed={onClaimed} />,
  );
  return { onClaimed, onClose };
}

// 测试 i18n 资源是空的：t(key) 原样返回 key —— 文案断言用 key 定位
// （与本仓其它组件测试同一约定），语义断言看行为副作用（发码 / 开窗 / 关窗）。

describe("StarRewardDialog", () => {
  it("确认 → 开浏览器仓库页 + 当场发码：展示优惠码、落 claimed、开注册窗", async () => {
    openExternal.mockResolvedValue(undefined);
    markClaimed.mockResolvedValue(undefined);
    openRegisterWindow.mockResolvedValue(undefined);
    const { onClaimed } = renderDialog();

    fireEvent.click(screen.getByText("loongport.star.confirm"));

    // 发码是同步的：点击那一刻码就在屏上，没有任何中间等待。
    expect(screen.getByText("LOONGPORT5")).toBeInTheDocument();
    expect(openExternal).toHaveBeenCalledWith(
      "https://github.com/SailingLoong/LoongPort",
    );
    expect(markClaimed).toHaveBeenCalled();
    await waitFor(() =>
      expect(openRegisterWindow).toHaveBeenCalledWith("LOONGPORT5"),
    );
    expect(onClaimed).toHaveBeenCalled();
  });

  it("浏览器打不开也照发码（开仓库页失败不挡荣誉制发码）", () => {
    openExternal.mockRejectedValue(new Error("no default browser"));
    markClaimed.mockResolvedValue(undefined);
    openRegisterWindow.mockResolvedValue(undefined);
    renderDialog();

    fireEvent.click(screen.getByText("loongport.star.confirm"));

    expect(screen.getByText("LOONGPORT5")).toBeInTheDocument();
    expect(markClaimed).toHaveBeenCalled();
  });

  it("取消 → 只关窗，不发码不开窗，红点保留（onClaimed 不触发）", () => {
    const { onClose, onClaimed } = renderDialog();
    fireEvent.click(screen.getByText("loongport.star.cancel"));
    expect(onClose).toHaveBeenCalled();
    expect(openExternal).not.toHaveBeenCalled();
    expect(openRegisterWindow).not.toHaveBeenCalled();
    expect(markClaimed).not.toHaveBeenCalled();
    expect(onClaimed).not.toHaveBeenCalled();
  });
});
