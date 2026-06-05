import { Logger } from './logger';
import { UserRepository } from './user-repository';

export interface User {
    id: string;
    name: string;
    email: string;
}

/**
 * Service layer for user operations.
 * Coordinates between the repository and business logic.
 */
export class UserService {
    private repo: UserRepository;
    private logger: Logger;

    constructor(logger: Logger) {
        this.logger = logger;
        this.repo = new UserRepository();
    }

    getUser(id: string): User | null {
        this.logger.info(`Fetching user: ${id}`);
        return this.repo.findById(id);
    }

    createUser(name: string, email: string): User {
        this.logger.info(`Creating user: ${name}`);
        const user: User = {
            id: `user-${Date.now()}`,
            name,
            email,
        };
        this.repo.save(user);
        return user;
    }

    listUsers(): User[] {
        return this.repo.findAll();
    }

    deleteUser(id: string): boolean {
        this.logger.info(`Deleting user: ${id}`);
        return this.repo.delete(id);
    }
}
