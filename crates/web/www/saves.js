// Battery saves, kept in the browser's IndexedDB.
//
// One record per cartridge, keyed by a name the emulator derives from the
// cartridge header. The value is the raw contents of the cartridge RAM, the
// same bytes a `.sav` file holds in other emulators.

const DB_NAME = "glassbit";
const STORE = "saves";

let dbPromise = null;

function openDb() {
  dbPromise ??= new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return dbPromise;
}

function run(mode, operation) {
  return openDb().then(
    (db) =>
      new Promise((resolve, reject) => {
        const transaction = db.transaction(STORE, mode);
        const request = operation(transaction.objectStore(STORE));
        transaction.oncomplete = () => resolve(request.result);
        transaction.onerror = () => reject(transaction.error);
        transaction.onabort = () => reject(transaction.error);
      }),
  );
}

/** Returns the stored save as a Uint8Array, or undefined when there is none. */
export function readSave(key) {
  return run("readonly", (store) => store.get(key));
}

/** Stores a save (a Uint8Array), replacing any previous one. */
export function writeSave(key, bytes) {
  return run("readwrite", (store) => store.put(bytes, key));
}
