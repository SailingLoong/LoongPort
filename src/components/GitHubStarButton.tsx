import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { GithubIcon } from "@/components/icons/GithubIcon";

/** The ordinary repository link in the application header. */
export function GitHubStarButton({ onClick }: { onClick: () => void }) {
  const { t } = useTranslation();
  const title = t("loongport.star.buttonTitle");

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      title={title}
      aria-label={title}
      onClick={onClick}
      className="hover:bg-black/5 dark:hover:bg-white/5"
    >
      <GithubIcon className="h-4 w-4" />
    </Button>
  );
}
