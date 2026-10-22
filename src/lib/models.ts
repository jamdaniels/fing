import { t } from "./i18n";
import type { ModelInfo, ModelVariant } from "./types";

export function formatModelSize(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

export function modelQualityLabel(variant: ModelVariant): string {
  return t(`models.quality.${variant}`);
}

/** One-line note under a model's name (only Parakeet has one). */
export function renderModelNote(model: ModelInfo): string {
  return model.engine === "parakeet"
    ? `<div class="engine-note">${t("models.parakeetNote")}</div>`
    : "";
}
