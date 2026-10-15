import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { GitHubStarButton } from "@/components/GitHubStarButton";

describe("GitHubStarButton", () => {
  it("shows the ordinary repository action without a reward indicator", () => {
    render(<GitHubStarButton onClick={vi.fn()} />);

    const button = screen.getByRole("button", {
      name: "loongport.star.buttonTitle",
    });
    expect(button).toHaveAttribute("title", "loongport.star.buttonTitle");
    expect(button).toBeEnabled();
    expect(button.querySelector(".bg-red-500")).toBeNull();
  });

  it("delegates one click to the repository action", () => {
    const onClick = vi.fn();
    render(<GitHubStarButton onClick={onClick} />);

    fireEvent.click(
      screen.getByRole("button", { name: "loongport.star.buttonTitle" }),
    );

    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
