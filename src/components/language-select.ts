import { Check, Plus, Search, X } from "lucide";
import { t } from "../lib/i18n";
import { createIcon, escapeHtml } from "../lib/icons";
import {
  languageName,
  WHISPER_LANGUAGES,
  type WhisperLanguage,
} from "../lib/languages";

export interface LanguageSelectOptions {
  selected: string[];
  onChange: (next: string[]) => void;
}

function filterLanguages(query: string): readonly WhisperLanguage[] {
  const q = query.trim().toLowerCase();
  if (!q) {
    return WHISPER_LANGUAGES;
  }
  return WHISPER_LANGUAGES.filter(
    (lang) =>
      lang.name.toLowerCase().includes(q) ||
      lang.nativeName.toLowerCase().includes(q) ||
      lang.code.startsWith(q)
  );
}

/**
 * Multi-select for transcription languages: selected languages as removable
 * chips plus an "Add language" chip opening a searchable popover.
 * Replaces the container's content and manages its own listeners.
 */
export function mountLanguageSelect(
  container: HTMLElement,
  options: LanguageSelectOptions
): void {
  let selected = [...options.selected];
  let highlight = 0;

  container.innerHTML = `
    <div class="langsel">
      <div class="langsel-chips"></div>
      <div class="langsel-pop" hidden>
        <div class="langsel-search">${createIcon(Search)}<input type="text" placeholder="${t("languageSelect.search")}" /></div>
        <div class="langsel-list"></div>
        <div class="langsel-foot"></div>
      </div>
    </div>
  `;
  const root = container.querySelector(".langsel") as HTMLElement;
  const chipRow = root.querySelector(".langsel-chips") as HTMLElement;
  const pop = root.querySelector(".langsel-pop") as HTMLElement;
  const searchInput = pop.querySelector("input") as HTMLInputElement;
  const list = pop.querySelector(".langsel-list") as HTMLElement;
  const foot = pop.querySelector(".langsel-foot") as HTMLElement;

  function renderChips(): void {
    const last = selected.length === 1;
    const removeTitle = escapeHtml(
      t(last ? "languageSelect.atLeastOne" : "common.remove")
    );
    chipRow.innerHTML = `${selected
      .map(
        (code) => `
          <span class="lang-chip on removable ${last ? "last" : ""}">
            ${escapeHtml(languageName(code))}
            <button type="button" class="langsel-chip-x" data-remove="${code}" title="${removeTitle}">${createIcon(X)}</button>
          </span>`
      )
      .join(
        ""
      )}<button type="button" class="lang-chip add-chip" data-add>${createIcon(Plus)} ${t("languageSelect.add")}</button>`;
  }

  function renderList(): void {
    const items = filterLanguages(searchInput.value);
    if (items.length === 0) {
      list.innerHTML = `<div class="langsel-empty">${t("languageSelect.empty")}</div>`;
    } else {
      list.innerHTML = items
        .map(
          (lang, i) => `
            <button type="button" class="langsel-opt ${selected.includes(lang.code) ? "selected" : ""} ${i === highlight ? "hl" : ""}" data-code="${lang.code}">
              <span class="code">${lang.code}</span>
              <span class="name">${escapeHtml(lang.nativeName)}</span>
              ${lang.name === lang.nativeName ? "" : `<span class="name-en">${escapeHtml(lang.name)}</span>`}
              <span class="check">${createIcon(Check)}</span>
            </button>`
        )
        .join("");
      list
        .querySelector(".langsel-opt.hl")
        ?.scrollIntoView({ block: "nearest" });
    }
    foot.textContent = t("languageSelect.counter", {
      selected: selected.length,
      shown: items.length,
      total: WHISPER_LANGUAGES.length,
    });
  }

  function isOpen(): boolean {
    return !pop.hidden;
  }

  function positionPop(): void {
    const anchor = chipRow.getBoundingClientRect();
    const margin = 8;
    pop.style.left = `${Math.max(margin, Math.min(anchor.left, window.innerWidth - pop.offsetWidth - margin))}px`;
    const openUp =
      anchor.bottom + margin + pop.offsetHeight + margin > window.innerHeight;
    if (openUp) {
      pop.style.top = "auto";
      pop.style.bottom = `${window.innerHeight - anchor.top + margin}px`;
    } else {
      pop.style.top = `${anchor.bottom + margin}px`;
      pop.style.bottom = "auto";
    }
  }

  function openPop(): void {
    root.classList.add("open");
    // Move the popover to <body> while open: ancestors with backdrop-filter or
    // a transform (e.g. the frosted onboarding card) become the containing
    // block for position:fixed and would hijack its viewport coordinates.
    document.body.appendChild(pop);
    pop.hidden = false;
    searchInput.value = "";
    highlight = 0;
    renderList();
    positionPop();
    searchInput.focus();
    document.addEventListener("mousedown", onOutsideDown);
    // Keep the popover anchored while an ancestor scrolls or the window resizes.
    document.addEventListener("scroll", positionPop, true);
    window.addEventListener("resize", positionPop);
  }

  function closePop(): void {
    root.classList.remove("open");
    pop.hidden = true;
    root.appendChild(pop);
    document.removeEventListener("mousedown", onOutsideDown);
    document.removeEventListener("scroll", positionPop, true);
    window.removeEventListener("resize", positionPop);
  }

  function onOutsideDown(e: MouseEvent): void {
    const target = e.target as Node;
    if (!(root.contains(target) || pop.contains(target))) {
      closePop();
    }
  }

  function toggleLanguage(code: string): void {
    if (selected.includes(code)) {
      if (selected.length === 1) {
        return;
      }
      selected = selected.filter((c) => c !== code);
    } else {
      selected = [...selected, code];
    }
    renderChips();
    if (isOpen()) {
      renderList();
      positionPop();
      searchInput.focus();
    }
    options.onChange([...selected]);
  }

  chipRow.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    const removeBtn = target.closest("[data-remove]") as HTMLElement | null;
    if (removeBtn?.dataset.remove) {
      toggleLanguage(removeBtn.dataset.remove);
      return;
    }
    if (target.closest("[data-add]")) {
      if (isOpen()) {
        closePop();
      } else {
        openPop();
      }
    }
  });

  list.addEventListener("mousedown", (e) => {
    const opt = (e.target as HTMLElement).closest(
      ".langsel-opt"
    ) as HTMLElement | null;
    if (opt?.dataset.code) {
      e.preventDefault();
      toggleLanguage(opt.dataset.code);
    }
  });

  searchInput.addEventListener("input", () => {
    highlight = 0;
    renderList();
  });

  searchInput.addEventListener("keydown", (e) => {
    const items = filterLanguages(searchInput.value);
    if (e.key === "ArrowDown") {
      e.preventDefault();
      highlight = Math.min(highlight + 1, items.length - 1);
      renderList();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      highlight = Math.max(highlight - 1, 0);
      renderList();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[highlight];
      if (item) {
        toggleLanguage(item.code);
      }
    } else if (e.key === "Escape") {
      e.stopPropagation();
      closePop();
    }
  });

  renderChips();
}
