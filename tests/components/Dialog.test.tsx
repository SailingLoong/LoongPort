import { render, screen } from "@testing-library/react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";

describe("Dialog", () => {
  it("renders the default modal layer above the application chrome", () => {
    render(
      <Dialog open>
        <DialogContent overlayClassName="test-dialog-overlay">
          <DialogTitle>Test dialog</DialogTitle>
          <DialogDescription>Test description</DialogDescription>
        </DialogContent>
      </Dialog>,
    );

    expect(screen.getByRole("dialog")).toHaveClass("z-[80]");
    expect(document.querySelector(".test-dialog-overlay")).toHaveClass(
      "z-[80]",
    );
  });
});
