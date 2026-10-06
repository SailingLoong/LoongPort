import { fireEvent } from "@testing-library/react";
import {
  initializeInputModality,
  isKeyboardModality,
} from "@/lib/inputModality";

beforeAll(() => initializeInputModality());
afterEach(() => {
  delete document.documentElement.dataset.keyboard;
});

it("recognizes navigation keys and resets to pointer modality on pointer down", () => {
  expect(isKeyboardModality()).toBe(false);
  fireEvent.keyDown(window, { key: "Tab" });
  expect(isKeyboardModality()).toBe(true);
  fireEvent.pointerDown(window);
  expect(isKeyboardModality()).toBe(false);
  fireEvent.keyDown(window, { key: "ArrowRight" });
  expect(isKeyboardModality()).toBe(true);
});

it("does not treat typing, activation or shortcuts as keyboard navigation", () => {
  for (const key of ["a", "Enter", " ", "Escape"])
    fireEvent.keyDown(window, { key });
  fireEvent.keyDown(window, { key: "Home", ctrlKey: true });
  fireEvent.keyDown(window, { key: "End", metaKey: true });
  fireEvent.keyDown(window, { key: "ArrowRight", altKey: true });
  expect(isKeyboardModality()).toBe(false);
});
