// One-off backfill: derive `country` for every existing job document from its `location`
// text, so the Somalia-first lists work on rows that were ingested before the field existed.
//
//   mongosh jobify --quiet scripts/backfill-country.js
//
// THIS IS A MIRROR of the mapping in `src/country.rs` — the ingest path derives the country
// with the Rust code, and this script only exists to bring the rows already in the database
// up to date without a deploy. If the Rust table changes before this is run, change this one
// too; after the backfill it should never be needed again (a re-scrape re-derives country
// from the same location text).
//
// It writes `country` only: `location` is never rewritten, and a document already carrying a
// country is recomputed from its location so the two can never disagree.

const SOMALI_PLACES = [
  'somalia', 'soomaaliya', 'somali republic',
  'somaliland', 'puntland', 'jubaland', 'galmudug', 'hirshabelle',
  'south west', 'southwest', 'south west state', 'banadir', 'benadir',
  'mogadishu', 'muqdisho', 'hargeisa', 'hargeysa', 'bosaso', 'boosaaso',
  'kismayo', 'kismaayo', 'baidoa', 'baydhabo', 'galkayo', 'gaalkacyo',
  'garowe', 'garoowe', 'beledweyne', 'belet weyne', 'dhuusamareeb',
  'dhusamareeb', 'dhusamareb', 'jowhar', 'joohar', 'marka', 'merka',
  'baraawe', 'brava', 'berbera', 'burao', 'burco', 'las anod',
  'laascaanood', 'lascanood', 'lasanod', 'baardheere', 'bardhere',
  'afgooye', 'afgoye', 'hudur', 'dhobley', 'dhoobley', 'dobley',
  'jamaame', 'jilib', 'guriel', 'sheekh', 'heliwa', 'baledogle', 'balad',
  'qardho', 'eyl', 'lasqoray', 'isle of somalia',
];

const FOREIGN_PLACES = [
  ['djibouti', 'dj'], ['jibouti', 'dj'], ['tadjourah', 'dj'],
  ['ethiopia', 'et'], ['ethiopian', 'et'], ['addis ababa', 'et'], ['addis', 'et'],
  ['jigjiga', 'et'], ['jijiga', 'et'], ['harar', 'et'], ['dire dawa', 'et'],
  ['kenya', 'ke'], ['kenyan', 'ke'], ['nairobi', 'ke'], ['mombasa', 'ke'], ['kisumu', 'ke'],
  ['uganda', 'ug'], ['ugandan', 'ug'], ['kampala', 'ug'],
  ['tanzania', 'tz'], ['dar es salaam', 'tz'],
  ['rwanda', 'rw'], ['kigali', 'rw'], ['burundi', 'bi'],
  ['south sudan', 'ss'], ['sudan', 'sd'], ['khartoum', 'sd'],
  ['egypt', 'eg'], ['cairo', 'eg'],
  ['yemen', 'ye'], ['sanaa', 'ye'], ['aden', 'ye'],
  ['saudi arabia', 'sa'], ['riyadh', 'sa'],
  ['dubai', 'ae'], ['abu dhabi', 'ae'], ['united arab emirates', 'ae'],
  ['qatar', 'qa'], ['doha', 'qa'], ['kuwait', 'kw'],
  ['jordan', 'jo'], ['amman', 'jo'], ['lebanon', 'lb'], ['beirut', 'lb'],
  ['iraq', 'iq'], ['baghdad', 'iq'], ['syria', 'sy'], ['damascus', 'sy'],
  ['turkey', 'tr'], ['ankara', 'tr'], ['istanbul', 'tr'],
  ['united kingdom', 'gb'], ['london', 'gb'],
  ['united states', 'us'], ['washington', 'us'], ['new york', 'us'],
  ['canada', 'ca'], ['ottawa', 'ca'],
  ['switzerland', 'ch'], ['geneva', 'ch'],
  ['belgium', 'be'], ['brussels', 'be'],
  ['netherlands', 'nl'], ['the hague', 'nl'], ['amsterdam', 'nl'],
  ['germany', 'de'], ['berlin', 'de'], ['france', 'fr'], ['paris', 'fr'],
  ['italy', 'it'], ['rome', 'it'], ['spain', 'es'], ['madrid', 'es'],
  ['sweden', 'se'], ['stockholm', 'se'], ['norway', 'no'], ['oslo', 'no'],
  ['denmark', 'dk'], ['copenhagen', 'dk'], ['poland', 'pl'], ['warsaw', 'pl'],
  ['ukraine', 'ua'], ['russia', 'ru'], ['moscow', 'ru'],
  ['australia', 'au'], ['japan', 'jp'], ['tokyo', 'jp'],
  ['china', 'cn'], ['beijing', 'cn'], ['india', 'in'], ['new delhi', 'in'],
  ['pakistan', 'pk'], ['islamabad', 'pk'], ['bangladesh', 'bd'], ['dhaka', 'bd'],
  ['afghanistan', 'af'], ['kabul', 'af'], ['nigeria', 'ng'], ['abuja', 'ng'],
  ['ghana', 'gh'], ['accra', 'gh'], ['senegal', 'sn'], ['dakar', 'sn'],
  ['south africa', 'za'], ['johannesburg', 'za'],
];

function normalise(value) {
  if (!value) return '';
  return String(value)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, ' ')
    .trim();
}

function contains(text, place) {
  return (' ' + text + ' ').indexOf(' ' + place + ' ') !== -1;
}

function countryOf(location) {
  const text = normalise(location);
  if (!text) return null;
  for (const place of SOMALI_PLACES) if (contains(text, place)) return 'so';
  for (const [place, code] of FOREIGN_PLACES) if (contains(text, place)) return code;
  return null;
}

const ops = [];
db.jobs.find({}, { location: 1 }).forEach((job) => {
  ops.push({
    updateOne: {
      filter: { _id: job._id },
      update: { $set: { country: countryOf(job.location) } },
    },
  });
});

for (let i = 0; i < ops.length; i += 500) {
  db.jobs.bulkWrite(ops.slice(i, i + 500), { ordered: false });
}

print('jobs seen: ' + ops.length + ', batches written: ' + Math.ceil(ops.length / 500));
print('--- counts by country ---');
db.jobs
  .aggregate([{ $group: { _id: '$country', n: { $sum: 1 } } }, { $sort: { n: -1, _id: 1 } }])
  .forEach((row) => print((row._id === null ? 'null' : row._id) + ': ' + row.n));
