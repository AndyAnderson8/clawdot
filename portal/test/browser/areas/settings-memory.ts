import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function memory(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, section, shot, step } = c;
  await step("Memory (Settings): add, refuse, forget one, forget all after a confirmation, off state", async () => {
    await open("open", "#/settings/memory");
    await b.until(`T.rows("#notelist > li").length === 1`, "one memory");
    await shot("settings-memory");
    await eq(`T.rows("#notelist > li")[0].includes("my printer is <b>Pixel</b>")`, true, "shown literally");
    await eq(`T.all("#panel #notelist b").length`, 0, "no markup from a memory");
    await eq(`T.q("#notedialog").open`, false, "the add form is not in the way");
    await T(`click("#addnote")`);
    await b.until(`T.q("#notedialog").open === true`, "the dialog opens");
    await T(`set("#note-text", "the spare key is under the pot")`);
    await T(`submit("#noteform")`);
    await b.until(`T.rows("#notelist > li").length === 2`, "added");
    assert.deepEqual(mock.log.filter((l) => l.method === "POST" && l.path === "/api/notes").pop()!.body, {
      text: "the spare key is under the pot",
    });
    await b.until(`T.q("#notedialog").open === false`, "the dialog closes once it is saved");
    await T(`click("#addnote")`);
    await T(`set("#note-text", "the key is sk-abc")`);
    await T(`submit("#noteform")`);
    await b.until(`T.text("#noteform .errtext").includes("password or a key")`, "a refusal is shown in the form");
    await eq(`T.rows("#notelist > li").length`, 2, "nothing was added");
    await T(`clickText("#notedialog", "Cancel")`);
    // the pencil changes a memory in place
    await T(`click("#notelist > li:nth-child(2) button[title='Change this memory']")`);
    await b.until(
      `T.q("#notedialog").open === true && T.q("#note-text").value === "the spare key is under the pot"`,
      "the pencil opens it with its words",
    );
    await eq(`T.text("#notedialog h3")`, "Change this memory", "and says it is a change");
    await T(`set("#note-text", "the spare key is in the shed")`);
    await T(`submit("#noteform")`);
    await b.until(
      `T.rows("#notelist > li")[1].includes("in the shed") && T.q("#notedialog").open === false`,
      "changed in place",
    );
    assert.equal(
      mock.log.filter((l) => l.method === "PUT" && l.path.startsWith("/api/notes/")).pop()!.path,
      "/api/notes/2",
    );
    await eq(`T.all("#notelist > li button").length`, 4, "each memory has two buttons: a pencil and an X");
    // with more than a few memories there is a search, answered by the device
    for (const t of [
      "the bins go out on Thursday",
      "mom's birthday is June 3",
      "the router is in the hall",
      "I like oolong tea",
    ])
      (mock.state["notes"] as { id: number; text: string; ts: number }[]).push({ id: 50 + t.length, text: t, ts: 1 });
    await b.ev(`location.hash = '#/dashboard'`);
    await section("memory", "Memory");
    await b.until(`!!T.q('#notesearch')`, "a search box once there are many");
    await T(`set('#notesearch', 'birthday')`);
    await b.until(
      `T.rows("#notelist > li").length === 1 && T.rows("#notelist > li")[0].includes("June 3")`,
      "the match",
    );
    await eq(`T.text('#notehits')`, "1 of 6 match", "and how many matched");
    // changing a memory that is on show updates the search result too
    await T(`click("#notelist > li:nth-child(1) button[title='Change this memory']")`);
    await T(`set("#note-text", "mom's birthday is June 4")`);
    await T(`submit("#noteform")`);
    await b.until(`T.rows("#notelist > li")[0]?.includes("June 4") === true`, "the search shows the changed text");
    assert.ok(
      mock.log.some((l) => l.method === "GET" && l.path === "/api/notes"),
      "the device was asked",
    );
    await T(`set('#notesearch', 'zzzz')`);
    await b.until(`!!T.q('#notenone')`, "nothing matches, and it says so");
    await T(`set('#notesearch', '')`);
    await b.until(`T.rows("#notelist > li").length === 6`, "all again");
    (mock.state["notes"] as unknown[]).splice(2);
    await b.ev(`location.hash = '#/dashboard'`);
    await section("memory", "Memory");
    await b.until(`T.rows("#notelist > li").length === 2`, "back to two");
    // forgetting asks first: declining does nothing
    await b.ev(`window.__answer = () => false`);
    await T(`click("#notelist > li:nth-child(1) button[title='Forget this memory']")`);
    await eq(`T.rows("#notelist > li").length`, 2, "declined confirmation keeps the memory");
    await b.ev(`window.__answer = () => true`);
    await T(`click("#notelist > li:nth-child(1) button[title='Forget this memory']")`);
    await b.until(`T.rows("#notelist > li").length === 1`, "one forgotten");
    assert.ok(mock.log.some((l) => l.method === "DELETE" && l.path === "/api/notes/1"));
    await T(`click("#forgetall")`);
    await b.until(`!!T.q("#noteempty")`, "all forgotten");
    assert.ok(mock.log.some((l) => l.method === "DELETE" && l.path === "/api/notes"));
    mock.state["notesOn"] = false;
    await b.ev(`location.hash = "#/dashboard"`);
    await section("memory", "Memory");
    await b.until(`!!T.q("#notesoff")`, "the off notice");
    await eq(`T.q("#notesoff a").getAttribute("href")`, "#/settings/features", "it says where to turn it on");
    await eq(`T.q("#addnote").disabled`, true, "adding is disabled while memory is off");
  });
}
