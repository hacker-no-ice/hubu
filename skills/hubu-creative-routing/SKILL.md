---
name: hubu-creative-routing
description: Choose which image model to use for a creative task under Hubu, and when to spend at all. Use whenever an agent is about to generate, edit or upscale an image through Hubu-governed execution, or must decide between its client's built-in image tool and a paid Gongbu target.
---

# Hubu creative routing

Pick the cheapest option that can do the job, spend through Hubu only when a
specialist model is needed, and move from drafts to finals deliberately. This
skill decides **which** target to use. It never grants spend authority; Hubu
policy, budgets and human approvals still decide whether a request runs.

## 1. Discover what is actually available

Call `gongbu_list_execution_targets` before choosing. Use only targets it
lists, with the sizes and prices it reports. Never assume a model exists
because this skill names it, and never construct a target ID.

If no listed target fits the task, say so and stop. Do not substitute a
weaker model silently.

## 2. Free first

If your client has a built-in image tool covered by the user's subscription,
use it for early exploration when quality is good enough. Spend through Hubu
when the task needs something the built-in tool can't do well:

- real vector output (SVG logos, icons);
- precise typography or long in-image text;
- photorealism or consistency across a series;
- blending several reference images, or editing a specific existing image;
- a size, aspect ratio or resolution the built-in tool doesn't offer.

Tell the user in one line why you are switching to a paid model.

## 3. Match the intent to a model family

Prefer the first listed family that fits. These are capability hints, not
guarantees; confirm against the target list.

| Intent | Prefer | Why |
| --- | --- | --- |
| Logo, icon, illustration that must scale | Recraft (vector) | Outputs SVG, not pixels |
| Poster, banner, thumbnail with text | Ideogram, then GPT Image | Strongest typography |
| Photoreal product or scene; consistent series | FLUX (Max or Pro) | Realism and cross-image consistency |
| Combine several references (character, product, style) | Gemini Pro Image | Multi-reference blending |
| Many quick drafts | FLUX Klein, Gemini Flash-Lite or Flash | Lowest cost per image |
| Edit, vary or upscale an existing image | Any target advertising editing for that workload | Keep the edit on the same artifact |

## 4. Draft, then final

1. **Draft** at the smallest listed size with the cheapest fitting target.
   Make a few variations, not dozens.
2. **Show** the drafts to the user with each one's settled cost, and let them
   pick. Do not pick a final on your own unless the user delegated that.
3. **Final**: regenerate or edit the chosen draft at the needed size, on the
   higher-quality target if it matters. Before submitting, state the expected
   cost from the target's listed price.
4. **Save** finals only to destinations the user has named.

## 5. Respect budgets and decisions

- Check remaining budget before a batch. If the batch would exhaust it, ask
  first or shrink the batch.
- `approval_required` means wait. Tell the user what is pending and continue
  only through `hubu_resume_operation` on the same handle after approval.
- A **denial is final** for that request. Never split a request into smaller
  ones, switch to an equivalent target, or lower the size just to get under a
  policy limit. Report the denial and ask the user how to proceed.
- Never rerun a completed operation to recover its image. Use the operation
  handle and artifact IDs.

## 6. Report what you spent

After each step, give the user a short line per output: model, size and
settled cost, plus the running total for the task.
