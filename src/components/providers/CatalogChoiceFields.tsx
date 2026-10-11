import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import type { ProviderCatalogDecision } from "@/lib/api/providers";

interface CatalogChoiceFieldsProps {
  form?: string | null;
  choice: ProviderCatalogDecision["choice"];
  disabled: boolean;
  onChange: (choice: ProviderCatalogDecision["choice"]) => void;
  onPreviewChoice: (choice: ProviderCatalogDecision["choice"]) => void;
  onCancel: () => void;
}

/** Backend-owned eligibility; these controls only collect an uncommitted choice. */
export function CatalogChoiceFields({
  form = "provider-form",
  choice,
  disabled,
  onChange,
  onPreviewChoice,
  onCancel,
}: CatalogChoiceFieldsProps) {
  const { t } = useTranslation();
  return (
    <fieldset disabled={disabled} className="mt-4 space-y-3">
      <legend className="text-body font-medium">
        {t("provider.preview.catalog.title")}
      </legend>
      {(["keep", "manage"] as const).map((value) => (
        <label
          key={value}
          className="flex items-start gap-2 rounded-panel border border-border-strong p-3 text-body"
        >
          <input
            type="radio"
            name="provider-catalog-choice"
            aria-label={t(`provider.preview.catalog.${value}`)}
            checked={choice === value}
            onChange={() => onChange(value)}
            className="mt-0.5 h-4 w-4 accent-primary"
          />
          <span>
            {t(`provider.preview.catalog.${value}`)}
            <span className="block text-caption text-fg-2">
              {t(`provider.preview.catalog.${value}Hint`)}
            </span>
          </span>
        </label>
      ))}
      <p className="text-caption text-fg-2">
        {t("provider.preview.catalog.previewHint")}
      </p>
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" onClick={onCancel}>
          {t("provider.preview.catalog.cancelChoice")}
        </Button>
        <Button
          type={form ? "submit" : "button"}
          form={form ?? undefined}
          onClick={() => onPreviewChoice(choice)}
        >
          {t("provider.preview.catalog.confirmChoice")}
        </Button>
      </div>
    </fieldset>
  );
}
