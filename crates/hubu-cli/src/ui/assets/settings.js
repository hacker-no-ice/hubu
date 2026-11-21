"use strict";

(() => {
  const backLink = document.getElementById("back-to-console");
  const from = new URLSearchParams(location.search).get("from");
  if (from?.startsWith("#/")) backLink.href = `/${from}`;

  const view = document.getElementById("view");

  async function changePermission(setting, enabled, button, feedback, close) {
    button.disabled = true;
    feedback.hidden = true;
    try {
      await ownerPost("/governance/overrides", {
        action: setting.action,
        enabled,
        acknowledge_warning: enabled,
      });
      close?.();
      await renderSettings();
    } catch (error) {
      feedback.textContent = error.message;
      feedback.hidden = false;
    } finally {
      button.disabled = false;
    }
  }

  function showAddPermission(available) {
    const permission = h("p", { hidden: true });
    const risk = h("p", { class: "notice bad", hidden: true });
    const feedback = h("div", { class: "notice bad", role: "alert", hidden: true });
    const select = h("select", { id: "permission-to-add" },
      h("option", { value: "" }, "Choose a permission"),
      available.map((setting) => h("option", { value: setting.action }, setting.label)));
    openDialog("Add a permission", h("div", {},
      h("div", { class: "permission-picker" },
        h("label", { for: select.id }, "Permission"), select), permission, risk, feedback), (close) => {
      const confirm = h("button", { type: "button", class: "danger", disabled: true,
        onclick: async () => {
          const setting = available.find((item) => item.action === select.value);
          if (!setting) return;
          select.disabled = true;
          await changePermission(setting, true, confirm, feedback, close);
          select.disabled = false;
        },
      }, "I understand, add permission");
      select.addEventListener("change", () => {
        const setting = available.find((item) => item.action === select.value);
        permission.textContent = setting?.permission || "";
        risk.textContent = setting?.risk || "";
        permission.hidden = !setting;
        risk.hidden = !setting;
        confirm.disabled = !setting;
        feedback.hidden = true;
      });
      return confirm;
    });
  }

  async function renderSettings() {
    clear(view).append(h("p", { class: "hint" }, "Loading…"));
    try {
      const { strict_permissions: strict, overrides } = await hubu("/governance/overrides");
      const granted = overrides.filter((setting) => setting.enabled);
      const available = overrides.filter((setting) => !setting.enabled);
      const feedback = h("div", { class: "notice bad", role: "alert", hidden: true });
      const add = available.length ? h("button", { type: "button",
        onclick: () => showAddPermission(available),
      }, "Add permission…") : null;
      const additional = granted.length
        ? h("ul", { class: "granted-permissions" }, granted.map((setting) => {
          const remove = h("button", { type: "button", class: "ghost",
            "aria-label": `Remove permission: ${setting.label}`,
            onclick: () => changePermission(setting, false, remove, feedback),
          }, "Remove");
          return h("li", {}, h("div", {}, h("strong", {}, setting.label),
            h("p", { class: "hint" }, setting.permission)), remove);
        }))
        : h("div", { class: "empty" }, "No additional permissions granted.");

      clear(view).append(
        h("div", { class: "settings-intro" }, h("h2", {}, "Agent action permissions"),
          h("p", { class: "hint" }, "Permissions apply to all agents in this Hubu profile."),
          h("p", { class: "hint" }, "Adding or removing an extra permission requires confirmation on your Mac.")),
        card("Allowed in Strict", h("ul", { class: "strict-permissions" },
          strict.map((label) => h("li", {}, label)))),
        feedback,
        card("Additional permissions", additional, add, true),
      );
    } catch (error) {
      if (error.status === 401) { showSignIn(); return; }
      clear(view).append(h("div", { class: "notice bad", role: "alert" }, error.message));
    }
  }

  renderSettings();
})();
