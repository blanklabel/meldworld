package model

const (
	ENTITY                  string = "ENTITY" // Generally used to update an entity
	ENTITYACTIONMOVE        string = "MOVE"
	ENTITYACTIONATTACK      string = "ATTACK"
	ENTITYACTIONRANGEATTACK string = "ATTACK.RANGE"
)

const (
	ENTITYDIRECTIONUP    string = "UP"
	ENTITYDIRECTIONDOWN  string = "DOWN"
	ENTITYDIRECTIONLEFT  string = "LEFT"
	ENTITYDIRECTIONRIGHT string = "RIGHT"
)

// Entity trying to do a thing
type EntityAction struct {
	ModelType
	Entity
	Action string // move, attack, attack.range
	EntityMove
}

// Entity movement
type EntityMove struct {
	Direction string // up down left right
	Distance  int    //
}

// Different status types
type Status struct {
	// TO DO
}

// Everything is an entity
type Entity struct {
	ModelType
	ID          string
	Owner       string
	Name        string
	Full_hp     int
	C_hp        int
	Phy_def     int
	Phy_atk     int
	Speed       int // tiles per tick
	Coordinates Cords
	Destination Cords
}

// Actors will be things that can attack or be attacked...
type Actor interface {
	ReduceHP(int)
	IncreaseHP(int)
	IsDead() bool
	GetStatuses() []Status
	Attack() Actor
	Move(Cords)
	GetCurrentHP() int
	GetMaxHP() int
	GetDefense() int
	GetAttack() int
	GetLocation() Cords
}
