import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { StarRewardOffer } from "@/lib/api";
import { StarRewardDialog } from "@/components/StarRewardDialog";

const { starViaGh, starCount, openRegisterWindow, openExternal, markClaimed } =
  vi.hoisted(() => ({
    starViaGh: vi.fn(),
    starCount: vi.fn(),
    openRegisterWindow: vi.fn(),
    openExternal: vi.fn(),
    markClaimed: vi.fn(),
  }));

vi.mock("@/lib/api", () => ({
  settingsApi: { openExternal },
  starRewardApi: { starViaGh, starCount, openRegisterWindow, markClaimed },
}));
vi.mock("@/lib/clipboard", () => ({
  copyText: vi.fn().mockResolvedValue(undefined),
}));

const offer: StarRewardOffer = {
  promoCode: "LOONGPORT5",
  amountUsd: 5,
  baselineStars: 100,
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
  it("确认 → gh 直点成功 → 展示优惠码、落 claimed、开注册窗", async () => {
    starViaGh.mockResolvedValue(true);
    markClaimed.mockResolvedValue(undefined);
    openRegisterWindow.mockResolvedValue(undefined);
    const { onClaimed } = renderDialog();

    fireEvent.click(screen.getByText("loongport.star.confirm"));

    await waitFor(() => {
      expect(screen.getByText("LOONGPORT5")).toBeInTheDocument();
    });
    expect(starCount).not.toHaveBeenCalled();
    expect(openExternal).not.toHaveBeenCalled();
    expect(markClaimed).toHaveBeenCalled();
    expect(openRegisterWindow).toHaveBeenCalledWith("LOONGPORT5");
    await waitFor(() => expect(onClaimed).toHaveBeenCalled());
  });

  it("gh 不通 → 开浏览器等待 → 我已点赞且星数涨了 → 发码", async () => {
    starViaGh.mockResolvedValue(false);
    openExternal.mockResolvedValue(undefined);
    starCount.mockResolvedValue(101); // 基线 100 → +1
    markClaimed.mockResolvedValue(undefined);
    openRegisterWindow.mockResolvedValue(undefined);
    renderDialog();

    fireEvent.click(screen.getByText("loongport.star.confirm"));

    await waitFor(() => {
      expect(openExternal).toHaveBeenCalledWith(
        "https://github.com/SailingLoong/LoongPort",
      );
    });
    fireEvent.click(screen.getByText("loongport.star.waitAction"));

    await waitFor(() => {
      expect(screen.getByText("LOONGPORT5")).toBeInTheDocument();
    });
    expect(openRegisterWindow).toHaveBeenCalledWith("LOONGPORT5");
  });

  it("我已点赞但星数没涨 → 仍发码（网络波动口径）", async () => {
    starViaGh.mockResolvedValue(false);
    openExternal.mockResolvedValue(undefined);
    starCount.mockResolvedValue(100); // 与基线相同
    markClaimed.mockResolvedValue(undefined);
    openRegisterWindow.mockResolvedValue(undefined);
    renderDialog();

    fireEvent.click(screen.getByText("loongport.star.confirm"));
    await waitFor(() => screen.getByText("loongport.star.waitAction"));
    fireEvent.click(screen.getByText("loongport.star.waitAction"));

    await waitFor(() => {
      expect(screen.getByText("LOONGPORT5")).toBeInTheDocument();
    });
    expect(screen.getByText("loongport.star.grantedUnverified")).toBeTruthy();
  });

  it("取消 → 只关窗，不发码不开窗", () => {
    const { onClose } = renderDialog();
    fireEvent.click(screen.getByText("loongport.star.cancel"));
    expect(onClose).toHaveBeenCalled();
    expect(starViaGh).not.toHaveBeenCalled();
    expect(openRegisterWindow).not.toHaveBeenCalled();
    expect(markClaimed).not.toHaveBeenCalled();
  });
});
