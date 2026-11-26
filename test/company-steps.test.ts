/**
 * The three company steps: name → services → city.
 *
 * A Discord modal holds text inputs only, so the city is a select menu and the draft has to survive
 * between interactions. These tests pin the order, the placeholders the owner asked for, and that
 * the write happens ONCE, at the city — never a half-saved company.
 */
import assert from 'node:assert/strict';
import test from 'node:test';

import { RequestGuard } from '../src/context.js';
import type { BotContext } from '../src/context.js';
import { COMPANY_CITIES, companyDrafts } from '../src/company.js';
import type { Backend } from '../src/http.js';
import { handleInteraction } from '../src/interactions.js';
import { OnboardingSessions } from '../src/onboarding.js';

const silentLogger = {
  info: () => undefined,
  warn: () => undefined,
  error: () => undefined,
  debug: () => undefined,
} as unknown as BotContext['logger'];

/** discord.js narrows by these predicates, so every fake has to answer them. */
function shape(kind: 'button' | 'modal' | 'select') {
  return {
    isChatInputCommand: () => false,
    isButton: () => kind === 'button',
    isStringSelectMenu: () => kind === 'select',
    isModalSubmit: () => kind === 'modal',
  };
}

interface Seen {
  content?: string;
  components?: unknown[];
  /** The modal as Discord receives it: `custom_id` keys, not the builder's camelCase. */
  modal?: { custom_id: string; components: { components: { custom_id: string; placeholder?: string }[] }[] };
}

function makeCtx(): { ctx: BotContext; writes: Record<string, unknown>[] } {
  const writes: Record<string, unknown>[] = [];
  const backend = {
    getUser: async () => ({ user: { name: 'Dayib', company: null, match_profile: null } }),
    setCompany: async (input: Record<string, unknown>) => {
      writes.push(input);
      return {
        user: { company: { name: input.name, services: input.services, location: input.location } },
        company_set: true,
      };
    },
  };
  const ctx = {
    backend: backend as unknown as Backend,
    appBaseUrl: 'https://jobs.example',
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    sessions: new OnboardingSessions(),
    guard: new RequestGuard(),
    logger: silentLogger,
  } as unknown as BotContext;
  return { ctx, writes };
}

function modalInputs(modal: Seen['modal']) {
  return (modal?.components ?? []).flatMap((row) =>
    row.components.map((c) => ({ customId: c.custom_id, placeholder: c.placeholder })),
  );
}

test('company setup asks name, then services, then a Somali city — and writes once', async () => {
  const { ctx, writes } = makeCtx();
  const seen: Seen = {};
  const snowflake = '424242424242424242';

  // Step 1 — the click opens the name modal, nothing is written.
  const open = {
    ...shape('button'),
    customId: 'company:open',
    user: { id: snowflake },
    showModal: async (modal: { toJSON: () => Seen['modal'] }) => {
      seen.modal = modal.toJSON();
    },
  };
  await handleInteraction(open as never, ctx);
  assert.equal(seen.modal?.custom_id, 'company:submit:name');
  assert.deepEqual(modalInputs(seen.modal), [{ customId: 'name', placeholder: 'SOOHSOFT LTD' }]);
  assert.equal(writes.length, 0, 'no write before the city is chosen');
  assert.deepEqual(companyDrafts.get(snowflake), { name: undefined });

  // Step 1 submit — the name is kept as a draft and step 2 is one button away.
  const nameSubmit = {
    ...shape('modal'),
    customId: 'company:submit:name',
    user: { id: snowflake },
    fields: { getTextInputValue: () => '  SOOHSOFT LTD ' },
    reply: async (payload: { content: string; components: unknown[] }) => {
      seen.content = payload.content;
      seen.components = payload.components;
    },
  };
  await handleInteraction(nameSubmit as never, ctx);
  assert.match(seen.content ?? '', /1 of 3 — company name saved:\*\* SOOHSOFT LTD/);
  assert.equal(writes.length, 0);
  assert.deepEqual(companyDrafts.get(snowflake), { name: 'SOOHSOFT LTD' });

  // Step 2 — the button opens the services modal with the requested placeholder.
  const servicesButton = {
    ...shape('button'),
    customId: 'company:services',
    user: { id: snowflake },
    showModal: async (modal: { toJSON: () => Seen['modal'] }) => {
      seen.modal = modal.toJSON();
    },
  };
  await handleInteraction(servicesButton as never, ctx);
  assert.equal(seen.modal?.custom_id, 'company:submit:services');
  assert.deepEqual(modalInputs(seen.modal), [
    { customId: 'services', placeholder: 'Construction, Logistics' },
  ]);

  // Step 2 submit — services parsed, the city menu is the reply, still no write.
  const servicesSubmit = {
    ...shape('modal'),
    customId: 'company:submit:services',
    user: { id: snowflake },
    fields: { getTextInputValue: () => 'Construction, Logistics' },
    reply: async (payload: { content: string; components: unknown[] }) => {
      seen.content = payload.content;
      seen.components = payload.components;
    },
  };
  await handleInteraction(servicesSubmit as never, ctx);
  assert.match(seen.content ?? '', /3 of 3/);
  assert.deepEqual(companyDrafts.get(snowflake), {
    name: 'SOOHSOFT LTD',
    services: ['Construction', 'Logistics'],
  });
  assert.equal(writes.length, 0, 'still nothing written before the city');

  const row = (seen.components ?? [])[0] as {
    toJSON: () => { components: { custom_id: string; options: { label: string; value: string }[] }[] };
  };
  const menu = row.toJSON().components[0];
  assert.equal(menu.custom_id, 'company:city');
  assert.deepEqual(
    menu.options.map((o) => o.value),
    [...COMPANY_CITIES],
    'every Somali city is offered, spelled one way',
  );
  assert.ok(COMPANY_CITIES.length <= 25, 'Discord caps a select menu at 25 options');

  // Step 3 — the city completes the profile in one write.
  const citySubmit = {
    ...shape('select'),
    customId: 'company:city',
    user: { id: snowflake },
    values: ['Hargeisa'],
    deferUpdate: async () => undefined,
    editReply: async (payload: { content: string }) => {
      seen.content = payload.content;
    },
  };
  await handleInteraction(citySubmit as never, ctx);
  assert.deepEqual(writes, [
    {
      discord_id: snowflake,
      name: 'SOOHSOFT LTD',
      services: ['Construction', 'Logistics'],
      location: 'Hargeisa',
    },
  ]);
  assert.match(seen.content ?? '', /Saved: \*\*SOOHSOFT LTD\*\* — Construction, Logistics, Hargeisa/);
  assert.equal(companyDrafts.get(snowflake), undefined, 'the draft is cleared after the write');
});

test('a city press with no draft answers without writing', async () => {
  const { ctx, writes } = makeCtx();
  const seen: Seen = {};
  const orphan = {
    ...shape('select'),
    customId: 'company:city',
    user: { id: '999' },
    values: ['Bosaso'],
    update: async (payload: { content: string }) => {
      seen.content = payload.content;
    },
  };
  await handleInteraction(orphan as never, ctx);
  assert.equal(writes.length, 0);
  assert.match(seen.content ?? '', /expired/);
});
