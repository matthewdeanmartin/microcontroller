export function reconcileSeeds<T extends {host:string;sources:string[];evidence?:string;kind?:string;ip?:string;lastSeen?:string}>(devices:T[],seeds:{host:string;sources:string[]}[]):T[];
