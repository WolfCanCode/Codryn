import { User } from './user-service';

/**
 * Repository layer for user persistence.
 * Handles CRUD operations against the data store.
 */
export class UserRepository {
    private users: Map<string, User> = new Map();

    findById(id: string): User | null {
        return this.users.get(id) || null;
    }

    findAll(): User[] {
        return Array.from(this.users.values());
    }

    save(user: User): void {
        this.users.set(user.id, user);
    }

    delete(id: string): boolean {
        return this.users.delete(id);
    }
}
