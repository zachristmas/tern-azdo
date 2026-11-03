//! `tern-azdo --demo`: a made-up organization, so the client can be tried
//! (and shown) without an Azure DevOps account. Nothing touches the network.

use serde_json::{Value, json};

use crate::api::{Detail, Identity, Pr, WorkItem};

fn ago(hours: i64) -> String {
	(chrono::Utc::now() - chrono::Duration::hours(hours)).to_rfc3339()
}

fn who(name: &str) -> Value {
	json!({ "displayName": name, "id": name.to_lowercase().replace(' ', ".") })
}

fn reviewer(name: &str, vote: i32, required: bool) -> Value {
	json!({ "displayName": name, "id": name.to_lowercase().replace(' ', "."), "vote": vote, "isRequired": required })
}

pub const ME: &str = "Alex Rivera";

pub fn me() -> Identity {
	serde_json::from_value(who(ME)).unwrap_or_default()
}

pub fn prs() -> Vec<Pr> {
	let pr = |id: u32, project: &str, repo: &str, title: &str, author: &str, hours: i64, src: &str, tgt: &str, draft: bool, reviewers: Value| {
		json!({
			"pullRequestId": id,
			"title": title,
			"description": DESCRIPTION,
			"createdBy": who(author),
			"creationDate": ago(hours),
			"isDraft": draft,
			"sourceRefName": format!("refs/heads/{src}"),
			"targetRefName": format!("refs/heads/{tgt}"),
			"repository": { "id": format!("{project}-{repo}"), "name": repo, "project": { "id": project, "name": project } },
			"reviewers": reviewers,
			"mergeStatus": "succeeded"
		})
	};
	let list = json!([
		pr(4821, "Storefront", "checkout-web", "Show tax per line on the order review page", "Priya Shah", 5, "feature/line-tax-display", "main", false,
			json!([reviewer(ME, 0, true), reviewer("Jordan Lee", 10, false), reviewer("Sam Okafor", 0, false)])),
		pr(4817, "Storefront", "checkout-web", "Remember the shipping method between visits", "Jordan Lee", 30, "feature/remember-shipping", "main", false,
			json!([reviewer(ME, 5, false), reviewer("Priya Shah", 10, false)])),
		pr(4809, "Storefront", "catalog-api", "Cache category trees for five minutes", "Sam Okafor", 52, "perf/category-cache", "main", false,
			json!([reviewer(ME, 0, false), reviewer("Mei Tanaka", -5, true)])),
		pr(4823, "Payments", "payments-service", "Retry card captures that time out, with an idempotency key", ME, 3, "fix/capture-timeout-retry", "release/2026.11", false,
			json!([reviewer("Mei Tanaka", 10, true), reviewer("Jordan Lee", 0, false)])),
		pr(4811, "Payments", "payments-service", "Move the PSP keys to Key Vault references", ME, 75, "chore/keyvault-psp", "main", true,
			json!([reviewer("Sam Okafor", 0, false)])),
		pr(4802, "Payments", "ledger", "Round half to even when posting FX differences", "Mei Tanaka", 120, "fix/fx-rounding", "main", false,
			json!([reviewer(ME, -10, false), reviewer("Priya Shah", 10, false), reviewer("Jordan Lee", 10, false)])),
		pr(4826, "Platform", "infra", "Split the build agents pool by OS", "Chris Novak", 1, "ops/agent-pools", "main", false,
			json!([reviewer(ME, 0, false)])),
		pr(4790, "Platform", "shared-ui", "Dark mode tokens for the admin portal", "Priya Shah", 260, "feature/dark-tokens", "main", true,
			json!([reviewer("Chris Novak", 0, false)])),
	]);
	serde_json::from_value(list).unwrap_or_default()
}

const DESCRIPTION: &str = "## What changes\n\nThe order review page now shows the tax for each line next to its price, \
and the summary splits tax by jurisdiction. Totals are unchanged: the per-line amounts come from the same \
calculation the order already makes.\n\n## How I checked\n\n- Unit tests for the rounding of each line against the order total\n\
- Placed orders in the staging store for a taxable and an exempt address\n\n## Rollback\n\nTurn off `Checkout.LineTax` in app settings.";

/// The demo's files: path, change type, old text, new text.
fn files() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
	vec![
		("/src/Checkout/OrderReview.vue", "edit", ORDER_REVIEW_OLD, ORDER_REVIEW_NEW),
		("/src/Checkout/TaxLine.vue", "add", "", TAX_LINE),
		("/src/Checkout/tax.ts", "edit", TAX_OLD, TAX_NEW),
		("/tests/Checkout/tax.spec.ts", "add", "", TAX_SPEC),
		("/config/appsettings.json", "edit", SETTINGS_OLD, SETTINGS_NEW),
	]
}

pub fn detail(pr: &Pr) -> Detail {
	let changes = files()
		.into_iter()
		.map(|(path, kind, _, _)| {
			serde_json::from_value(json!({
				"item": { "objectId": format!("new:{path}"), "originalObjectId": format!("old:{path}"), "path": path },
				"changeType": kind
			}))
			.expect("demo change")
		})
		.collect();
	let comment = |author: &str, text: &str| json!({ "author": who(author), "content": text, "commentType": "text" });
	let threads = json!([
		{ "id": 1, "status": "active",
		  "threadContext": { "filePath": "/src/Checkout/tax.ts", "rightFileStart": { "line": 14 } },
		  "comments": [
			comment("Jordan Lee", "Rounding each line and then summing can drift a cent from the order total. Should the last line absorb the difference?"),
			comment("Priya Shah", "Good catch. `allocate` now gives the remainder to the largest line, and the spec covers a three-line order that used to be off by one cent.")
		  ] },
		{ "id": 2, "status": "fixed",
		  "threadContext": { "filePath": "/src/Checkout/OrderReview.vue", "rightFileStart": { "line": 12 } },
		  "comments": [ comment("Sam Okafor", "Can the tax column hide on narrow screens? It wraps the price on phones.") ] },
		{ "id": 3, "status": "active",
		  "comments": [ comment("Jordan Lee", "Approved. The staging orders look right for both addresses.") ] }
	]);
	let _ = pr;
	Detail {
		changes,
		threads: serde_json::from_value(threads).unwrap_or_default(),
		work_items: vec![
			WorkItem { id: 1312, title: "Show line-level tax at checkout".into(), state: "Active".into(), kind: "User Story".into() },
			WorkItem { id: 1318, title: "Tax summary by jurisdiction".into(), state: "New".into(), kind: "Task".into() },
		],
	}
}

/// A demo blob's text: `old:<path>` or `new:<path>`.
pub fn blob(id: &str) -> Option<String> {
	let (side, path) = id.split_once(':')?;
	let (_, _, old, new) = files().into_iter().find(|(p, ..)| *p == path)?;
	Some(if side == "old" { old } else { new }.to_owned())
}

const ORDER_REVIEW_OLD: &str = r#"<template>
  <section class="order-review">
    <h2>Review your order</h2>
    <table class="lines">
      <tr v-for="line in order.lines" :key="line.id">
        <td class="name">{{ line.name }}</td>
        <td class="qty">{{ line.quantity }}</td>
        <td class="price">{{ money(line.total) }}</td>
      </tr>
    </table>
    <OrderTotals :order="order" />
  </section>
</template>

<script setup lang="ts">
import OrderTotals from './OrderTotals.vue'
import { money } from './format'

defineProps<{ order: Order }>()
</script>
"#;

const ORDER_REVIEW_NEW: &str = r#"<template>
  <section class="order-review">
    <h2>Review your order</h2>
    <table class="lines">
      <tr v-for="line in order.lines" :key="line.id">
        <td class="name">{{ line.name }}</td>
        <td class="qty">{{ line.quantity }}</td>
        <td class="price">{{ money(line.total) }}</td>
        <TaxLine v-if="lineTax" :tax="taxes[line.id]" />
      </tr>
    </table>
    <OrderTotals :order="order" :by-jurisdiction="lineTax" />
  </section>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import OrderTotals from './OrderTotals.vue'
import TaxLine from './TaxLine.vue'
import { money } from './format'
import { allocate } from './tax'
import { useSetting } from '../settings'

const props = defineProps<{ order: Order }>()
const lineTax = useSetting('Checkout.LineTax')
const taxes = computed(() => allocate(props.order.tax, props.order.lines))
</script>
"#;

const TAX_LINE: &str = r#"<template>
  <td class="tax">
    <span class="amount">{{ money(tax.amount) }}</span>
    <span class="rate">{{ percent(tax.rate) }}</span>
  </td>
</template>

<script setup lang="ts">
import { money, percent } from './format'

defineProps<{ tax: LineTax }>()
</script>

<style scoped>
.tax { color: var(--muted); white-space: nowrap; }
@media (max-width: 480px) { .tax .rate { display: none; } }
</style>
"#;

const TAX_OLD: &str = r#"export interface LineTax {
  amount: number
  rate: number
}

export function lineTax(total: number, rate: number): LineTax {
  return { amount: Math.round(total * rate * 100) / 100, rate }
}
"#;

const TAX_NEW: &str = r#"export interface LineTax {
  amount: number
  rate: number
}

export function lineTax(total: number, rate: number): LineTax {
  return { amount: Math.round(total * rate * 100) / 100, rate }
}

/// Splits the order's tax across its lines in proportion to each line's total.
/// Rounding each share can leave a cent over or short, so the largest line
/// takes the remainder and the shares always add up to the order's tax.
export function allocate(orderTax: number, lines: Line[]): Record<string, LineTax> {
  const subtotal = lines.reduce((sum, l) => sum + l.total, 0)
  const shares = lines.map(l => ({ id: l.id, amount: round2((orderTax * l.total) / subtotal), rate: l.taxRate }))
  const drift = round2(orderTax - shares.reduce((sum, s) => sum + s.amount, 0))
  const largest = shares.reduce((a, b) => (b.amount > a.amount ? b : a))
  largest.amount = round2(largest.amount + drift)
  return Object.fromEntries(shares.map(s => [s.id, { amount: s.amount, rate: s.rate }]))
}

const round2 = (n: number) => Math.round(n * 100) / 100
"#;

const TAX_SPEC: &str = r#"import { describe, expect, it } from 'vitest'
import { allocate } from '../../src/Checkout/tax'

describe('allocate', () => {
  it('adds up to the order tax when rounding drifts', () => {
    const lines = [
      { id: 'a', total: 10, taxRate: 0.0825 },
      { id: 'b', total: 10, taxRate: 0.0825 },
      { id: 'c', total: 10, taxRate: 0.0825 },
    ]
    const shares = allocate(2.48, lines)
    const sum = Object.values(shares).reduce((s, t) => s + t.amount, 0)
    expect(Math.round(sum * 100) / 100).toBe(2.48)
  })
})
"#;

const SETTINGS_OLD: &str = r#"{
  "Checkout": {
    "GuestCheckout": true,
    "SavedCards": true
  }
}
"#;

const SETTINGS_NEW: &str = r#"{
  "Checkout": {
    "GuestCheckout": true,
    "SavedCards": true,
    "LineTax": true
  }
}
"#;
