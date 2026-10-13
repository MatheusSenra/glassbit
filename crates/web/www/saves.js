// Battery saves and save states, kept in the browser's IndexedDB.
//
// Two stores, each with one record per cartridge, keyed by a name the
// emulator derives from the cartridge header:
//  - "saves" holds the raw contents of the cartridge RAM, the same bytes a
//    `.sav` file holds in other emulators;
//  - "states" holds a snapshot of the whole machine.

const DB_NAME = "glassbit";
const DB_VERSION = 2;
const SAVES = "saves";
const STATES = "states";

let dbPromise = null;

function openDb() {
  dbPromise ??= new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      // Runs for new databases and for ones created before states existed.
      for (const store of [SAVES, STATES]) {
        if (!request.result.objectStoreNames.contains(store)) {
          request.result.createObjectStore(store);
        }
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return dbPromise;
}

function run(storeName, mode, operation) {
  return openDb().then(
    (db) =>
      new Promise((resolve, reject) => {
        const transaction = db.transaction(storeName, mode);
        const request = operation(transaction.objectStore(storeName));
        transaction.oncomplete = () => resolve(request.result);
        transaction.onerror = () => reject(transaction.error);
        transaction.onabort = () => reject(transaction.error);
      }),
  );
}

/** Returns the stored battery save as a Uint8Array, or undefined. */
export function readSave(key) {
  return run(SAVES, "readonly", (store) => store.get(key));
}

/** Stores a battery save (a Uint8Array), replacing any previous one. */
export function writeSave(key, bytes) {
  return run(SAVES, "readwrite", (store) => store.put(bytes, key));
}

/** Returns the stored save state as a Uint8Array, or undefined. */
export function readState(key) {
  return run(STATES, "readonly", (store) => store.get(key));
}

/** Stores a save state (a Uint8Array), replacing any previous one. */
export function writeState(key, bytes) {
  return run(STATES, "readwrite", (store) => store.put(bytes, key));
}
