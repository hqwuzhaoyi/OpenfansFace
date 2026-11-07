export const PRIVACY_STORAGE_KEY = 'wb-privacy'

// Applied before the body paints; inaccessible storage fails closed.
export const privacyInitScript = `(()=>{let v='on';try{v=localStorage.getItem('${PRIVACY_STORAGE_KEY}')==='on'?'on':'off'}catch{}document.documentElement.dataset.privacy=v})()`
