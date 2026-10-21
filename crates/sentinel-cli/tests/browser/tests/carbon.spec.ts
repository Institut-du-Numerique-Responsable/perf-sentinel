import { test, expect, Page } from "@playwright/test";

// Carbon tab: the Energy card names where the energy figure comes from,
// never the window energy_model (an intensity tag on Electricity Maps
// daemons). The rules mirror GreenSummary::energy_source_label. Total CO2
// carries co2.total.model, as the CLI's Est. CO2 line does.

type Row = [service: string, tag: string, ratio: number];

async function loadCarbon(
  page: Page,
  energyKwh: number,
  windowTag: string,
  rows: Row[],
  calibrated = false,
  co2Model?: string,
  patch?: (gs: any) => void,
) {
  await page.route("**/dashboard.html", async (route) => {
    const response = await route.fetch();
    const body = await response.text();
    const patched = body.replace(
      /(<script id="report-data" type="application\/json">\s*)([\s\S]*?)(\s*<\/script>)/,
      (_match, open, json, close) => {
        const payload = JSON.parse(json);
        const gs = payload.report.green_summary;
        gs.energy_kwh = energyKwh;
        gs.energy_model = windowTag;
        gs.energy_calibrated = calibrated;
        gs.per_service_energy_model = Object.fromEntries(rows.map(([s, t]) => [s, t]));
        gs.per_service_measured_ratio = Object.fromEntries(rows.map(([s, , r]) => [s, r]));
        if (co2Model !== undefined) gs.co2.total.model = co2Model;
        if (patch) patch(gs);
        return open + JSON.stringify(payload) + close;
      },
    );
    await route.fulfill({ response, body: patched });
  });
  await page.goto("/dashboard.html#green");
  await page.waitForSelector("#green-metrics .ps-metric");
}

function energyCard(page: Page) {
  return page.locator("#green-metrics > .ps-metric").nth(1);
}

async function expectEnergySub(page: Page, sub: string) {
  const card = energyCard(page);
  await expect(card.locator(".ps-metric-label")).toContainText("Energy");
  await expect(card.locator(".ps-metric-sub")).toHaveText(sub);
}

test("1. the Energy card sits right after Total CO2", async ({ page }) => {
  await loadCarbon(page, 0.0123, "io_proxy_v3", [["a", "io_proxy_v3", 0]]);
  const labels = page.locator("#green-metrics > .ps-metric .ps-metric-label");
  await expect(labels).toHaveCount(9);
  await expect(labels.nth(0)).toContainText("Total CO2");
  await expect(labels.nth(1)).toContainText("Energy");
  await expect(labels.nth(2)).toContainText("Operational CO2");
  await expect(energyCard(page).locator(".ps-metric-value")).toHaveText("0.012 kWh");
});

test("2. proxy-only energy reads as modeled from I/O counts", async ({ page }) => {
  await loadCarbon(page, 0.0123, "io_proxy_v3", [
    ["a", "io_proxy_v3", 0],
    ["b", "io_proxy_v3", 0],
  ]);
  await expectEnergySub(page, "modeled from I/O counts");
});

test("3. a mixed window names the backends and the covered share", async ({ page }) => {
  await loadCarbon(page, 0.5, "scaphandre_rapl", [
    ["a", "scaphandre_rapl", 1],
    ["b", "kepler_ebpf", 0.4],
    ["c", "io_proxy_v3", 0],
  ]);
  await expectEnergySub(
    page,
    "source kepler_ebpf, scaphandre_rapl on 2 of 3 services · rest modeled from I/O counts",
  );
});

test("4. full coverage names the backend once, without +cal", async ({ page }) => {
  await loadCarbon(page, 0.5, "scaphandre_rapl+cal", [
    ["a", "scaphandre_rapl+cal", 1],
    ["b", "scaphandre_rapl", 1],
  ]);
  await expectEnergySub(page, "source scaphandre_rapl");
});

test("5. a calibrated proxy says so", async ({ page }) => {
  await loadCarbon(page, 0.5, "io_proxy_v3+cal", [["a", "io_proxy_v3+cal", 0]]);
  await expectEnergySub(page, "modeled from I/O counts · calibrated");
});

test("5b. calibration behind a measured window tag still shows", async ({ page }) => {
  // The window tag drops +cal there, only energy_calibrated carries it.
  await loadCarbon(
    page,
    0.5,
    "scaphandre_rapl",
    [
      ["a", "scaphandre_rapl", 1],
      ["b", "scaphandre_rapl", 0],
    ],
    true,
  );
  await expectEnergySub(
    page,
    "source scaphandre_rapl on 1 of 2 services · rest modeled from I/O counts · calibrated",
  );
});

test("5c. calibration behind an Electricity Maps tag still shows", async ({ page }) => {
  await loadCarbon(page, 0.5, "electricity_maps_api", [["a", "electricity_maps_api", 0]], true);
  await expectEnergySub(page, "modeled from I/O counts · calibrated");
});

test("6. an Electricity Maps window tag is never shown as the energy source", async ({ page }) => {
  await loadCarbon(page, 0.5, "electricity_maps_api", [
    ["a", "electricity_maps_api", 0],
    ["b", "electricity_maps_api", 0],
  ]);
  await expectEnergySub(page, "modeled from I/O counts");
  await expect(energyCard(page)).not.toContainText("electricity_maps_api");
});

test("7. no computed energy greys the card out", async ({ page }) => {
  await loadCarbon(page, 0, "", []);
  const card = energyCard(page);
  await expect(card.locator(".ps-metric-value")).toHaveText("-");
  await expect(card.locator(".ps-metric-value")).toHaveAttribute("data-tone", "muted");
  await expectEnergySub(page, "not computed · no span resolved to a region");
});

test("8. hostile tags are dropped, never rendered", async ({ page }) => {
  const long = "x".repeat(65);
  const bidi = "kepler\u202E_ebpf";
  await loadCarbon(page, 0.5, "io_proxy_v3", [
    ["a", long, 1],
    ["b", bidi, 1],
    ["c", "redfish_bmc", 1],
  ]);
  await expectEnergySub(page, "source redfish_bmc");
  const text = await page.locator("#green-metrics").innerText();
  expect(text).not.toContain(long);
  expect(text).not.toContain("\u202E");
});

test("9. at 900px the ninth card spans both columns", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 900 });
  await loadCarbon(page, 0.5, "io_proxy_v3", [["a", "io_proxy_v3", 0]]);
  const cards = page.locator("#green-metrics > .ps-metric");
  await expect(cards).toHaveCount(9);
  await expect(cards.nth(8).locator(".ps-metric-label")).toContainText("Broker waste");
  const first = await cards.nth(0).boundingBox();
  const second = await cards.nth(1).boundingBox();
  const last = await cards.nth(8).boundingBox();
  // Two columns: the second card starts the right column on row 1.
  expect(second!.y).toBeCloseTo(first!.y, 0);
  expect(second!.x).toBeGreaterThan(first!.x);
  // The last card runs from the left column to the right edge.
  expect(last!.x).toBeCloseTo(first!.x, 0);
  expect(last!.x + last!.width).toBeCloseTo(second!.x + second!.width, 0);
});

function totalSub(page: Page) {
  return page.locator("#green-metrics > .ps-metric").nth(0).locator(".ps-metric-sub");
}

test("10. Total CO2 names the window carbon model after the bracket", async ({ page }) => {
  await loadCarbon(page, 0.5, "io_proxy_v3", [["a", "io_proxy_v3", 0]], false, "io_proxy_v3");
  await expect(totalSub(page)).toHaveText(/^low .+ · high .+ · model\sio_proxy_v3$/);
});

test("11. the carbon model keeps +cal and names a measured backend", async ({ page }) => {
  await loadCarbon(page, 0.5, "kepler_ebpf", [["a", "kepler_ebpf", 1]], true, "kepler_ebpf+cal");
  await expect(totalSub(page)).toContainText("· model kepler_ebpf+cal");
  await expectEnergySub(page, "source kepler_ebpf");
});

test("12. an Electricity Maps tag shows as the carbon model, not the energy source", async ({ page }) => {
  await loadCarbon(page, 0.5, "electricity_maps_api", [["a", "electricity_maps_api", 0]], false, "electricity_maps_api");
  await expect(totalSub(page)).toContainText("· model electricity_maps_api");
  await expectEnergySub(page, "modeled from I/O counts");
});

test("13. a hostile or missing carbon model leaves the bracket alone", async ({ page }) => {
  await loadCarbon(page, 0.5, "io_proxy_v3", [["a", "io_proxy_v3", 0]], false, "io\u202E_proxy");
  await expect(totalSub(page)).toHaveText(/^low .+ · high [^·]+$/);
  expect(await page.locator("#green-metrics").innerText()).not.toContain("\u202E");
  await page.unrouteAll();
  await loadCarbon(page, 0.5, "io_proxy_v3", [["a", "io_proxy_v3", 0]], false, "");
  await expect(totalSub(page)).not.toContainText("model");
});

test("14. the Overview Total CO2 card carries the same model tag", async ({ page }) => {
  await loadCarbon(page, 0.5, "io_proxy_v3", [["a", "io_proxy_v3", 0]], false, "io_proxy_v2+cal");
  await page.goto("/dashboard.html#overview");
  const card = page.locator("#findings-metrics > .ps-metric", { hasText: "Total CO2" });
  await expect(card.locator(".ps-metric-sub")).toContainText("· model io_proxy_v2+cal");
});

test("15. in-total hosts split the energy and get their own carbon card", async ({ page }) => {
  await loadCarbon(page, 0.5, "io_proxy_v3", [["a", "io_proxy_v3", 0]], false, undefined, (gs) => {
    gs.database_waste = {
      energy_kwh: 0.1, waste_kwh: 0.02, sql_waste_ratio: 0.2,
      model: "database_specpower", in_totals: true, placement: "dedicated",
    };
    gs.messaging_waste = {
      energy_kwh: 0.05, waste_kwh: 0, messaging_waste_ratio: 0, model: "alumet_rapl",
    };
    gs.co2.operational_gco2 = 3;
    gs.co2.database_gco2 = 1;
    gs.co2.avoidable.mid = 2;
  });
  await expectEnergySub(
    page,
    "modeled from I/O counts · plus database hosts, database_specpower = applications 0.400 kWh + database hosts 0.100 kWh",
  );
  const cards = page.locator("#green-metrics > .ps-metric");
  await expect(cards).toHaveCount(10);
  const byLabel = (label: string) =>
    cards.filter({ has: page.locator(".ps-metric-label", { hasText: new RegExp("^" + label) }) });
  const dbCard = byLabel("Database CO2");
  await expect(dbCard.locator(".ps-metric-sub")).toHaveText("host energy · model\u00a0database_specpower");
  await expect(page.locator("#green-metrics")).not.toContainText("Broker CO2");
  const avoidable = byLabel("Avoidable");
  await expect(avoidable.locator(".ps-metric-sub")).toHaveText("50% of operational and host");
  const dbWaste = byLabel("Database waste");
  await expect(dbWaste.locator(".ps-metric-sub")).toContainText("· dedicated hosts · host energy, in the report totals");
  const broker = byLabel("Broker waste");
  await expect(broker.locator(".ps-metric-sub")).toContainText("outside these totals");
});
